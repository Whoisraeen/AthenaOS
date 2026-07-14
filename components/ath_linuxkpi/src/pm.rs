//! Runtime + system power-management surface.
//!
//! The `amdgpud` daemon owns the GPU's power state directly — it holds the GFX
//! domain awake during bring-up via the SMU (DisallowGfxOff). Kernel runtime-PM
//! is therefore a no-op here: the device is **always active**. These return
//! success *truthfully* (the GPU really is powered when amdgpu asks), and the
//! suspend/hibernate queries report "not suspending" because the daemon never is.

use core::ffi::c_void;

type Dev = *mut c_void;

// ── runtime-PM core (the `__pm_runtime_*` out-of-line entry points) ──────────
#[no_mangle]
pub extern "C" fn __pm_runtime_disable(_dev: Dev, _check_resume: bool) {}
#[no_mangle]
pub extern "C" fn __pm_runtime_idle(_dev: Dev, _flags: i32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn __pm_runtime_resume(_dev: Dev, _flags: i32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn __pm_runtime_suspend(_dev: Dev, _flags: i32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn __pm_runtime_set_status(_dev: Dev, _status: u32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn __pm_runtime_use_autosuspend(_dev: Dev, _use: bool) {}

// ── runtime-PM inline wrappers the shim externs (get/put, queries) ───────────
#[no_mangle]
pub extern "C" fn pm_runtime_enable(_dev: Dev) {}
#[no_mangle]
pub extern "C" fn pm_runtime_allow(_dev: Dev) {}
#[no_mangle]
pub extern "C" fn pm_runtime_forbid(_dev: Dev) {}
#[no_mangle]
pub extern "C" fn pm_runtime_get_sync(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_resume_and_get(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_get_noresume(_dev: Dev) {}
#[no_mangle]
pub extern "C" fn pm_runtime_put(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_put_sync(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_put_autosuspend(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_put_noidle(_dev: Dev) {}
#[no_mangle]
pub extern "C" fn pm_runtime_mark_last_busy(_dev: Dev) {}
/// Device is always active in the daemon model → return > 0.
#[no_mangle]
pub extern "C" fn pm_runtime_get_if_active(_dev: Dev) -> i32 {
    1
}
#[no_mangle]
pub extern "C" fn pm_runtime_get_if_in_use(_dev: Dev) -> i32 {
    1
}
#[no_mangle]
pub extern "C" fn pm_runtime_autosuspend_expiration(_dev: Dev) -> u64 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_set_autosuspend_delay(_dev: Dev, _delay: i32) {}
#[no_mangle]
pub extern "C" fn pm_runtime_force_resume(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_force_suspend(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_barrier(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_runtime_suspended(_dev: Dev) -> bool {
    false
}
#[no_mangle]
pub extern "C" fn pm_runtime_active(_dev: Dev) -> bool {
    true
}

// ── generic power-domain (genpd) — no domains in the daemon model ────────────
#[no_mangle]
pub extern "C" fn pm_genpd_add_device(_genpd: Dev, _dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_genpd_remove_device(_dev: Dev) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pm_genpd_init(_genpd: Dev, _gov: Dev, _is_off: bool) -> i32 {
    0
}

// ── system suspend / hibernate queries — the daemon never suspends ───────────
#[no_mangle]
pub extern "C" fn pm_hibernate_is_recovering() -> bool {
    false
}
#[no_mangle]
pub extern "C" fn pm_hibernation_mode_is_suspend() -> bool {
    false
}

/// `pm_suspend_global_flags` — global suspend flags word; 0 = none set.
#[no_mangle]
pub static pm_suspend_global_flags: u32 = 0;
/// `pm_suspend_target_state` — current system suspend target; `PM_SUSPEND_ON` (0).
#[no_mangle]
pub static pm_suspend_target_state: u32 = 0;
