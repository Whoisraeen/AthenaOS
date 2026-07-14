//! kanata_daemon — userspace input-macro router for AthenaOS.
//!
//! Concept §Input: "user-defined macros / remaps, hold-tap, layers."
//! This daemon owns the kbd capability handle granted by the kernel,
//! drains scancodes from the kbd channel, runs them through a real
//! `kanata-keyberon` `Layout` state machine (hold-tap + layers), and
//! emits the resulting HID keycodes back to the system via `SYS_PRINT`
//! as a liveness/diagnostic sentinel. Routing the post-layout keycodes
//! back into the kernel's input router is the next slice — the engine
//! itself is real working code on the syscall path today.
//!
//! Licensing: `kanata-keyberon` is MIT (TeXitoi keyberon fork). The
//! `kanata-parser` crate that would consume `.kbd` config files is
//! LGPL-3.0; we vendor it but DO NOT link it from here until Phase 11
//! brings a `std` userspace. See `docs/THIRD_PARTY_LICENSES.md`.

#![no_std]
#![no_main]

extern crate alloc;

use kanata_keyberon::action::{k, Action, HoldTapAction, HoldTapConfig};
use kanata_keyberon::key_code::KeyCode;
use kanata_keyberon::layout::{Event, Layout};

#[allow(unused_imports)]
use raekit;

// ── Syscall numbers (kept in sync with kernel/src/syscall.rs) ─────────
const SYS_PRINT: u64 = 1;
const SYS_RECV: u64 = 3;
const SYS_EXIT: u64 = 12;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_PRINT, in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_EXIT, in("rdi") code,
        options(noreturn),
    );
}

#[inline(always)]
unsafe fn sys_recv(cap_handle: u64) -> (u64, u64, u64, u64, u64) {
    let status: u64;
    let msg_type: u64;
    let arg1: u64;
    let arg2: u64;
    let arg3: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_RECV => status,
        in("rdi") cap_handle,
        lateout("rsi") msg_type,
        lateout("rdx") arg1,
        lateout("r10") arg2,
        lateout("r8")  arg3,
        out("rcx") _, out("r11") _,
    );
    (status, msg_type, arg1, arg2, arg3)
}

// ── Layout table ──────────────────────────────────────────────────────
//
// 8-column × 1-row, 1 layer. Column 0 is the signature kanata example:
// hold CapsLock-physical-key for LeftCtrl, tap it for an actual Escape
// (the "Caps as Ctrl/Esc" remap that ~every kanata user installs first).
// Columns 1..=7 are straight A..=G passthrough so we can prove plain
// key events also propagate through the engine.

const CAPS_HOLD_TAP: HoldTapAction<'static, core::convert::Infallible> = HoldTapAction {
    on_press_reset_timeout_to: None,
    require_prior_idle: None,
    timeout: 200,
    hold: k(KeyCode::LCtrl),
    tap: k(KeyCode::Escape),
    timeout_action: k(KeyCode::LCtrl),
    config: HoldTapConfig::Default,
    tap_hold_interval: 0,
};

// Layers must outlive the Layout, so they're `static`.
static SRC_KEYS: [Action<'static, core::convert::Infallible>; 8] = [Action::NoOp; 8];
static LAYERS: [[[Action<'static, core::convert::Infallible>; 8]; 1]; 1] = [[[
    Action::HoldTap(&CAPS_HOLD_TAP),
    k(KeyCode::A),
    k(KeyCode::B),
    k(KeyCode::C),
    k(KeyCode::D),
    k(KeyCode::E),
    k(KeyCode::F),
    k(KeyCode::G),
]]];

/// Map a raw HID-style scancode to a column index in our 8-col layout.
/// Returns None for scancodes the daemon doesn't bind, so the routing
/// loop can just drop unknown events instead of panicking.
fn scancode_to_col(scancode: u8) -> Option<u16> {
    match scancode {
        0x39 => Some(0), // CapsLock
        0x04 => Some(1), // A
        0x05 => Some(2), // B
        0x06 => Some(3), // C
        0x07 => Some(4), // D
        0x08 => Some(5), // E
        0x09 => Some(6), // F
        0x0A => Some(7), // G
        _ => None,
    }
}

/// Encode a HID KeyCode as a `KEY<keycode>` magic value for sys_print so
/// we can read the resolved-key stream off the serial log without an
/// extra IPC channel. The 0x4B45000000 prefix is ASCII "KE\0\0\0".
fn encode_keycode(kc: KeyCode) -> u64 {
    0x4B45000000u64 | (kc as u64)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // Sentinel: 'KANATA' so the boot log shows the daemon got past _start.
        sys_print(0x4B414E415441);

        // Construct the layout engine. `trans_resolution_behavior_v2 = true`
        // and `delegate_to_first_layer = false` match kanata's default
        // user config.
        let mut layout: Layout<'static, 8, 1, core::convert::Infallible> =
            Layout::new_with_trans_action_settings(&SRC_KEYS, &LAYERS, true, false);

        // Track which scancodes are currently held so we can emit
        // matching Release events when the kernel reports a key-up.
        // 256-bit dense bitmap = 4 × u64.
        let mut held: [u64; 4] = [0; 4];

        let kbd_cap: u64 = 1;
        let mut ticks: u64 = 0;

        loop {
            let (status, _mtype, arg1, arg2, _arg3) = sys_recv(kbd_cap);
            if status == 0 {
                // arg1 = scancode (HID usage), arg2 = pressed flag (0 = release).
                let scancode = arg1 as u8;
                let pressed = arg2 != 0;
                let bit = 1u64 << (scancode & 63);
                let word = (scancode >> 6) as usize & 3;
                let was_held = (held[word] & bit) != 0;

                if let Some(col) = scancode_to_col(scancode) {
                    let evt = if pressed {
                        Event::Press(0, col)
                    } else {
                        Event::Release(0, col)
                    };
                    layout.event(evt);
                } else {
                    // Unmapped scancode — echo it through unchanged so
                    // the kernel input loop still sees the raw key.
                    sys_print(0x4B5200000000u64 | scancode as u64); // 'KR..'
                }

                if pressed && !was_held {
                    held[word] |= bit;
                }
                if !pressed && was_held {
                    held[word] &= !bit;
                }
            }

            // Always tick — hold-tap timing depends on tick count, not
            // just on incoming events. Every iteration of this loop is
            // one logical millisecond from the layout's point of view;
            // the actual wall-clock rate is whatever the kbd capability
            // delivers + the spin-pause below.
            let _ = layout.tick();
            ticks = ticks.wrapping_add(1);

            // Emit the active keycode set. In a real wiring this would
            // go through SYS_INPUT_INJECT (TBD); for today's proof we
            // dump each held keycode as KEY<kc> on the serial line so
            // boot.ps1 can grep for the resolved Ctrl/Esc behavior.
            for kc in layout.keycodes() {
                sys_print(encode_keycode(kc));
            }

            // Idle pacing if there was no event. Without this the daemon
            // would saturate a core spinning on sys_recv. 10k spin-loop
            // iterations approximates ~tens of microseconds on QEMU.
            if status != 0 {
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }
            }

            // Safety exit: if the daemon gets wedged with no kbd cap
            // (cap_handle was wrong), bail after a generous tick budget
            // instead of running forever and obscuring the log.
            if ticks > 1_000_000 {
                sys_print(0x4B41444F4E45u64); // 'KADONE'
                sys_exit(0);
            }
        }
    }
}

// Panic handler comes from `raekit` (the shared userspace runtime).
