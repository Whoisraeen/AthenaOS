//! Rae scripting layer — Concept §Customization Engine:
//!
//! > "Scripting layer — Swift scripts for automation, no PowerShell
//! >  archaeology required."
//!
//! Most automation stories on desktop OSes are awful: PowerShell on
//! Windows is its own forbidden cuneiform, AppleScript on macOS is half
//! deprecated, bash on Linux is great for power users and impossible for
//! everyone else. AthenaOS ships a single first-class scripting surface:
//! you write Rae script, the system runs it sandboxed under your
//! capability set, and you can invoke it from the shell, the Settings UI,
//! a keyboard shortcut, or a calendar trigger.
//!
//! The interpreter is `components/athlang` — a Swift-flavored, no_std,
//! fuel-limited tree-walker shared with the userspace `athlangd` daemon.
//! Scripts up to 64 KiB run INLINE at submit (fuel-capped: a runaway loop
//! ends in `Timeout`, never a hang); larger sources stay queued for the
//! daemon. The kernel also owns the **lifecycle**: registering a script,
//! tracking exit state + captured output, killing runaways. Same shape as
//! systemd-style unit management but with a simpler ABI because
//! everything has to fit in 5 syscall registers.
//!
//! ## System bindings (the automation surface)
//!
//! Inline scripts get a [`KernelHost`]: every call the script makes that
//! isn't script-defined lands here and is gated on the submitting user's
//! `cap_mask` (AthGuard model — deny by default, the user authorizes a
//! script's capability set at submit):
//!
//! | binding | cap bit | backs onto |
//! |---|---|---|
//! | `uptimeMs()` `wallClock()` `windowCount()` `osVersion()` | `SCRIPT_CAP_SYSINFO` | boot clock, tray clock, compositor |
//! | `notify(title)` | `SCRIPT_CAP_NOTIFY` | `notify::post` |
//! | `getAccent()` `setAccent(argb)` | `SCRIPT_CAP_THEME` | `theme_engine` |
//! | `getConfig(key)` `setConfig(key, v)` | `SCRIPT_CAP_CONFIG` | `config_registry` (versioned, snapshot/rollback) |
//! | `setWallpaper(name)` | `SCRIPT_CAP_WALLPAPER` | `live_wallpaper` |
//! | `launchApp(path)` | `SCRIPT_CAP_LAUNCH` | `shell_runner::spawn_app_from_vfs` |
//!
//! A denied call fails the whole script closed (`CapabilityDenied`), it
//! does not silently no-op.
//!
//! ## Syscalls (78-80, 294-295)
//!
//! | nr | name             | rdi/rsi/rdx                                          | rax |
//! |----|------------------|------------------------------------------------------|----|
//! | 78 | SCRIPT_RUN       | rdi=src_ptr, rsi=src_len, rdx=cap_mask               | script id |
//! | 79 | SCRIPT_STATUS    | rdi=script_id, rsi=out_ptr (ScriptAbi)               | bytes |
//! | 80 | SCRIPT_KILL      | rdi=script_id                                        | 0/err |
//! | 294 | SCRIPT_FETCH    | rdi=out_ptr, rsi=out_cap (ScriptJobAbi + source)     | bytes/0 |
//! | 295 | SCRIPT_COMPLETE | rdi=script_id, rsi=exit_code, rdx=out_ptr, r10=len   | 0/err |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ── State ──────────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued = 0,
    Running = 1,
    Completed = 2,
    Failed = 3,
    Killed = 4,
    Timeout = 5,
}

#[derive(Debug, Clone)]
struct Script {
    id: u64,
    source_hash: [u8; 8], // cheap fingerprint for /proc display
    cap_mask: u64,        // bitmask of capabilities the user authorized
    state: State,
    submitted_tsc: u64,
    finished_tsc: u64,
    exit_code: i32,
    /// Captured `print(...)` output (truncated to [`MAX_OUTPUT_BYTES`]).
    output: String,
    /// Retained source for daemon-run scripts (too large for inline);
    /// handed to `athlangd` by SCRIPT_FETCH, dropped once claimed.
    source: Option<Vec<u8>>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ScriptAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub state: u32,
    pub exit_code: i32,
    pub cap_mask: u64,
    pub submitted_tsc: u64,
    pub finished_tsc: u64,
    pub source_hash: [u8; 8],
}

struct Engine {
    scripts: BTreeMap<u64, Script>,
    total_runs: u64,
    total_failures: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    *ENGINE.lock() = Some(Engine {
        scripts: BTreeMap::new(),
        total_runs: 0,
        total_failures: 0,
    });
    crate::serial_println!(
        "[ OK ] Rae scripting layer: inline interpreter + capability-gated bindings ready",
    );
}

// ── Script capabilities (cap_mask bits) ────────────────────────────────
//
// The submitting surface (shell, palette, Settings, syscall caller)
// authorizes a script's system access as a bitmask. Deny by default: a
// cap_mask of 0 is pure computation.

pub const SCRIPT_CAP_SYSINFO: u64 = 1 << 0;
pub const SCRIPT_CAP_NOTIFY: u64 = 1 << 1;
pub const SCRIPT_CAP_THEME: u64 = 1 << 2;
pub const SCRIPT_CAP_CONFIG: u64 = 1 << 3;
pub const SCRIPT_CAP_WALLPAPER: u64 = 1 << 4;
pub const SCRIPT_CAP_LAUNCH: u64 = 1 << 5;
pub const SCRIPT_CAP_ALL: u64 = (1 << 6) - 1;

/// The kernel's [`athlang::Host`]: the system-API surface automation
/// scripts see, each name gated on the user-authorized `cap_mask`.
/// Concept §Customization Engine — scripts drive the SYSTEM (theme,
/// wallpaper, notifications, config), not just arithmetic.
struct KernelHost {
    cap_mask: u64,
}

impl KernelHost {
    fn require(&self, bit: u64, name: &str) -> Result<(), athlang::HostError> {
        if self.cap_mask & bit != 0 {
            Ok(())
        } else {
            Err(athlang::HostError::Denied(alloc::format!(
                "{}: capability not granted (cap_mask=0x{:x})",
                name,
                self.cap_mask
            )))
        }
    }
}

fn one_str_arg(name: &str, args: &[athlang::Value]) -> Result<String, athlang::HostError> {
    match args {
        [athlang::Value::Str(s)] => Ok(s.clone()),
        _ => Err(athlang::HostError::Failed(alloc::format!(
            "{} takes one String argument",
            name
        ))),
    }
}

fn one_int_arg(name: &str, args: &[athlang::Value]) -> Result<i64, athlang::HostError> {
    match args {
        [athlang::Value::Int(n)] => Ok(*n),
        _ => Err(athlang::HostError::Failed(alloc::format!(
            "{} takes one Int argument",
            name
        ))),
    }
}

impl athlang::Host for KernelHost {
    fn call(
        &mut self,
        name: &str,
        args: &[athlang::Value],
    ) -> Result<athlang::Value, athlang::HostError> {
        use athlang::{HostError, Value};
        match name {
            // ── sysinfo ──
            "uptimeMs" => {
                self.require(SCRIPT_CAP_SYSINFO, name)?;
                Ok(Value::Int(crate::boot_elapsed_ms() as i64))
            }
            "wallClock" => {
                self.require(SCRIPT_CAP_SYSINFO, name)?;
                Ok(Value::Str(crate::shell_runner::tray_clock_string()))
            }
            "windowCount" => {
                self.require(SCRIPT_CAP_SYSINFO, name)?;
                Ok(Value::Int(
                    crate::compositor::list_userspace_surfaces().len() as i64,
                ))
            }
            "osVersion" => {
                self.require(SCRIPT_CAP_SYSINFO, name)?;
                Ok(Value::Str(String::from("AthenaOS 0.0.1")))
            }
            // ── notifications ──
            "notify" => {
                self.require(SCRIPT_CAP_NOTIFY, name)?;
                let title = one_str_arg(name, args)?;
                let posted = crate::notify::post(
                    "script",
                    &title,
                    crate::shell_api::NotificationUrgency::Normal,
                );
                Ok(Value::Bool(posted))
            }
            // ── theme ──
            "getAccent" => {
                self.require(SCRIPT_CAP_THEME, name)?;
                Ok(Value::Int(crate::theme_engine::active_accent() as i64))
            }
            "setAccent" => {
                self.require(SCRIPT_CAP_THEME, name)?;
                let argb = one_int_arg(name, args)?;
                crate::theme_engine::set_active_accent(argb as u32);
                Ok(Value::Unit)
            }
            // ── versioned config (snapshot/rollback like all settings) ──
            "getConfig" => {
                self.require(SCRIPT_CAP_CONFIG, name)?;
                let key = one_str_arg(name, args)?;
                Ok(match crate::config_registry::get_text(&key) {
                    Some(v) => Value::Str(v),
                    None => Value::Unit,
                })
            }
            "setConfig" => {
                self.require(SCRIPT_CAP_CONFIG, name)?;
                match args {
                    [Value::Str(k), Value::Str(v)] => {
                        crate::config_registry::set_text(k, v);
                        Ok(Value::Unit)
                    }
                    _ => Err(HostError::Failed(String::from(
                        "setConfig takes (String key, String value)",
                    ))),
                }
            }
            // ── wallpaper ──
            "setWallpaper" => {
                self.require(SCRIPT_CAP_WALLPAPER, name)?;
                let wanted = one_str_arg(name, args)?;
                match crate::live_wallpaper::find_by_name(&wanted) {
                    Some(id) => Ok(Value::Bool(crate::live_wallpaper::set_current(id) == 0)),
                    None => Ok(Value::Bool(false)),
                }
            }
            // ── app launch ──
            "launchApp" => {
                self.require(SCRIPT_CAP_LAUNCH, name)?;
                let path = one_str_arg(name, args)?;
                crate::shell_runner::spawn_app_from_vfs(&path);
                Ok(Value::Unit)
            }
            _ => Err(HostError::Unknown),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// FNV-1a 64 truncated to 8 bytes. Just for /proc display + dedup, not security.
fn fingerprint(bytes: &[u8]) -> [u8; 8] {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h.to_le_bytes()
}

// ── Public APIs ────────────────────────────────────────────────────────

/// Scripts up to this size run INLINE through the in-kernel `athlang`
/// interpreter at submit time (fuel-capped, so a runaway terminates
/// deterministically). Larger sources stay Queued for the userspace
/// interpreter daemon.
const INLINE_MAX_BYTES: usize = 64 * 1024;
/// Interpreter fuel per inline script (statements + loop iterations).
const INLINE_FUEL: u64 = 1_000_000;
/// Captured-output ceiling per script.
const MAX_OUTPUT_BYTES: usize = 4 * 1024;

pub fn submit(source: &[u8], cap_mask: u64) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let inline = source.len() <= INLINE_MAX_BYTES;
    let script = Script {
        id,
        source_hash: fingerprint(source),
        cap_mask,
        state: State::Queued,
        submitted_tsc: rdtsc(),
        finished_tsc: 0,
        exit_code: 0,
        output: String::new(),
        // Only daemon-bound scripts keep their source in the engine.
        source: if inline { None } else { Some(source.to_vec()) },
    };
    {
        let mut g = ENGINE.lock();
        if let Some(e) = g.as_mut() {
            e.scripts.insert(id, script);
            e.total_runs += 1;
        }
    } // ENGINE released during execution — status() stays callable.

    if inline {
        // Rae script actually RUNS (Concept §Customization Engine): the
        // shared `athlang` interpreter, fuel-limited so `while true {}`
        // ends in Timeout instead of wedging the kernel, with the
        // cap_mask-gated KernelHost as its system-API surface.
        let mut host = KernelHost { cap_mask };
        let result = core::str::from_utf8(source)
            .map_err(|_| athlang::RaeError::Lex(String::from("source is not UTF-8")))
            .and_then(|src| athlang::run_with_host(src, INLINE_FUEL, &mut host));

        let mut g = ENGINE.lock();
        if let Some(e) = g.as_mut() {
            let failed = !matches!(result, Ok(_));
            let timed_out = matches!(result, Err(athlang::RaeError::OutOfFuel));
            if let Some(s) = e.scripts.get_mut(&id) {
                s.finished_tsc = rdtsc();
                match result {
                    Ok(outcome) => {
                        s.state = State::Completed;
                        s.exit_code = outcome.exit_code as i32;
                        s.output = truncate_output(outcome.output);
                    }
                    Err(err) => {
                        s.state = if timed_out {
                            State::Timeout
                        } else {
                            State::Failed
                        };
                        s.exit_code = -1;
                        s.output = alloc::format!("error: {:?}", err);
                    }
                }
            }
            if failed {
                e.total_failures += 1;
            }
        }
    }
    // else: stays Queued with retained source until athlangd fetches it.
    id
}

/// Next queued (daemon-bound) script, if any: claims it (Queued →
/// Running), hands back id + cap_mask + source. The daemon half of the
/// lifecycle — the kernel never runs >64 KiB sources inline.
pub fn fetch_next_queued() -> Option<(u64, u64, Vec<u8>)> {
    let mut g = ENGINE.lock();
    let e = g.as_mut()?;
    let id = e
        .scripts
        .iter()
        .find(|(_, s)| s.state == State::Queued && s.source.is_some())
        .map(|(id, _)| *id)?;
    let s = e.scripts.get_mut(&id)?;
    s.state = State::Running;
    let src = s.source.take()?;
    Some((id, s.cap_mask, src))
}

/// Daemon reports a finished script (exit_code < 0 marks failure).
pub fn complete(id: u64, exit_code: i64, output: &[u8]) -> u64 {
    let mut g = ENGINE.lock();
    let e = match g.as_mut() {
        Some(e) => e,
        None => return ERR_NOT_INIT,
    };
    let s = match e.scripts.get_mut(&id) {
        Some(s) => s,
        None => return ERR_NO_SUCH,
    };
    if s.state != State::Running {
        return ERR_BAD_STATE;
    }
    s.finished_tsc = rdtsc();
    s.exit_code = exit_code as i32;
    s.state = if exit_code < 0 {
        State::Failed
    } else {
        State::Completed
    };
    s.output = truncate_output(String::from_utf8_lossy(output).into_owned());
    if exit_code < 0 {
        e.total_failures += 1;
    }
    0
}

/// Cap captured output at [`MAX_OUTPUT_BYTES`] WITHOUT panicking on a
/// UTF-8 char boundary (String::truncate would).
fn truncate_output(mut s: String) -> String {
    if s.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Captured `print(...)` output of a finished script (for /proc + shell).
pub fn output_of(id: u64) -> Option<String> {
    ENGINE
        .lock()
        .as_ref()?
        .scripts
        .get(&id)
        .map(|s| s.output.clone())
}

pub fn kill(id: u64) -> u64 {
    let mut g = ENGINE.lock();
    let e = match g.as_mut() {
        Some(e) => e,
        None => return ERR_NOT_INIT,
    };
    let s = match e.scripts.get_mut(&id) {
        Some(s) => s,
        None => return ERR_NO_SUCH,
    };
    match s.state {
        State::Queued | State::Running => {
            s.state = State::Killed;
            s.finished_tsc = rdtsc();
            s.exit_code = -1;
            e.total_failures += 1;
            0
        }
        _ => ERR_BAD_STATE,
    }
}

pub fn status(id: u64) -> Option<ScriptAbi> {
    let g = ENGINE.lock();
    g.as_ref()?.scripts.get(&id).map(|s| ScriptAbi {
        version: 1,
        id: s.id,
        state: s.state as u32,
        exit_code: s.exit_code,
        cap_mask: s.cap_mask,
        submitted_tsc: s.submitted_tsc,
        finished_tsc: s.finished_tsc,
        source_hash: s.source_hash,
    })
}

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_FB01;
pub const ERR_NO_SUCH: u64 = 0xFFFF_FFFF_FFFF_FB02;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_FB03;
pub const ERR_BAD_STATE: u64 = 0xFFFF_FFFF_FFFF_FB04;
pub const ERR_TOO_LARGE: u64 = 0xFFFF_FFFF_FFFF_FB05;

// ── Syscalls ───────────────────────────────────────────────────────────

pub const SYS_SCRIPT_RUN: u64 = 78;
pub const SYS_SCRIPT_STATUS: u64 = 79;
pub const SYS_SCRIPT_KILL: u64 = 80;
pub const SYS_SCRIPT_FETCH: u64 = 294;
pub const SYS_SCRIPT_COMPLETE: u64 = 295;

/// SCRIPT_FETCH reply header, followed immediately by `source_len` bytes
/// of script source in the same buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ScriptJobAbi {
    pub version: u32, // = 1
    pub _pad: u32,
    pub id: u64,
    pub cap_mask: u64,
    pub source_len: u64,
}

const MAX_SRC_BYTES: u64 = 1_048_576; // 1 MiB script source ceiling

pub fn sys_run(
    src_ptr: u64,
    src_len: u64,
    cap_mask: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if src_len == 0 || src_len > MAX_SRC_BYTES {
        return ERR_TOO_LARGE;
    }
    if !validate_r(src_ptr, src_len, false) {
        return ERR_BAD_USER;
    }
    // SMAP-safe: validated extable copy-in (was a raw copy_nonoverlapping FROM
    // the user ptr).
    let buf = match crate::uaccess::copy_from_user(src_ptr, src_len as usize) {
        Ok(b) => b,
        Err(_) => return ERR_BAD_USER,
    };
    submit(&buf, cap_mask)
}

pub fn sys_status(
    id: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    let size = core::mem::size_of::<ScriptAbi>() as u64;
    if out_cap < size {
        return u64::MAX;
    }
    if !validate_w(out_ptr, out_cap, true) {
        return u64::MAX;
    }
    let abi = match status(id) {
        Some(a) => a,
        None => return u64::MAX,
    };
    // SMAP-safe: assemble the ScriptAbi struct + (optionally) the captured
    // output into one kernel buffer and do a single validated extable copy-out.
    // Additive extension: a buffer larger than ScriptAbi also receives the
    // captured `print` output right after the struct (how the shell's `rae`
    // command shows results). 56-byte callers are unaffected.
    debug_assert_eq!(size as usize, 56);
    let mut out_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(size as usize);
    out_buf.extend_from_slice(&abi.version.to_le_bytes());
    out_buf.extend_from_slice(&[0u8; 4]); // pad
    out_buf.extend_from_slice(&abi.id.to_le_bytes());
    out_buf.extend_from_slice(&abi.state.to_le_bytes());
    out_buf.extend_from_slice(&abi.exit_code.to_le_bytes());
    out_buf.extend_from_slice(&abi.cap_mask.to_le_bytes());
    out_buf.extend_from_slice(&abi.submitted_tsc.to_le_bytes());
    out_buf.extend_from_slice(&abi.finished_tsc.to_le_bytes());
    out_buf.extend_from_slice(&abi.source_hash);
    let mut written = size;
    if out_cap > size {
        if let Some(out) = output_of(id) {
            let bytes = out.as_bytes();
            let n = core::cmp::min(bytes.len() as u64, out_cap - size);
            out_buf.extend_from_slice(&bytes[..n as usize]);
            written += n;
        }
    }
    if crate::uaccess::copy_to_user(out_ptr, &out_buf).is_err() {
        return u64::MAX;
    }
    written
}

pub fn sys_kill(id: u64) -> u64 {
    kill(id)
}

/// SCRIPT_FETCH: `athlangd` claims the next queued (>64 KiB) script.
/// Returns bytes written (ScriptJobAbi + source), 0 when nothing is
/// queued, or an `ERR_*` code.
pub fn sys_fetch(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    let header = core::mem::size_of::<ScriptJobAbi>() as u64;
    if out_cap < header {
        return ERR_TOO_LARGE;
    }
    if !validate_w(out_ptr, out_cap, true) {
        return ERR_BAD_USER;
    }
    // Claim under the lock; copy to userspace after releasing it.
    let (id, cap_mask, src) = {
        let mut g = ENGINE.lock();
        let e = match g.as_mut() {
            Some(e) => e,
            None => return ERR_NOT_INIT,
        };
        let id = match e
            .scripts
            .iter()
            .find(|(_, s)| s.state == State::Queued && s.source.is_some())
            .map(|(id, _)| *id)
        {
            Some(id) => id,
            None => return 0,
        };
        let s = e.scripts.get_mut(&id).expect("id was just found");
        let need = header + s.source.as_ref().map(|v| v.len()).unwrap_or(0) as u64;
        if out_cap < need {
            return ERR_TOO_LARGE;
        }
        s.state = State::Running;
        let src = s.source.take().expect("queued script keeps its source");
        (id, s.cap_mask, src)
    };
    // SMAP-safe: assemble the ScriptJobAbi header + source into one kernel
    // buffer, one validated extable copy-out (was two raw user-ptr writes).
    debug_assert_eq!(header as usize, 32);
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(header as usize + src.len());
    out.extend_from_slice(&1u32.to_le_bytes()); // version
    out.extend_from_slice(&0u32.to_le_bytes()); // _pad
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&cap_mask.to_le_bytes());
    out.extend_from_slice(&(src.len() as u64).to_le_bytes());
    out.extend_from_slice(&src);
    if crate::uaccess::copy_to_user(out_ptr, &out).is_err() {
        return ERR_BAD_USER;
    }
    header + src.len() as u64
}

/// SCRIPT_COMPLETE: `athlangd` reports a finished script (exit_code is
/// signed via two's complement; negative marks Failed).
pub fn sys_complete(
    id: u64,
    exit_code: u64,
    out_ptr: u64,
    out_len: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    let len = core::cmp::min(out_len, MAX_OUTPUT_BYTES as u64);
    if len == 0 {
        return complete(id, exit_code as i64, &[]);
    }
    if !validate_r(out_ptr, len, false) {
        return ERR_BAD_USER;
    }
    // SMAP-safe: validated extable copy-in (was a raw copy_nonoverlapping FROM
    // the user ptr).
    let buf = match crate::uaccess::copy_from_user(out_ptr, len as usize) {
        Ok(b) => b,
        Err(_) => return ERR_BAD_USER,
    };
    complete(id, exit_code as i64, &buf)
}

// ── /proc/athena/scripts ────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = ENGINE.lock();
    let e = match g.as_ref() {
        Some(e) => e,
        None => return String::from("# scripting engine not initialized\n"),
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS scripting layer ({} scripts ever, {} failures, {} live)\n",
        e.total_runs,
        e.total_failures,
        e.scripts.len(),
    ));
    for (id, s) in &e.scripts {
        out.push_str(&alloc::format!(
            "#{:<4} state={:?} exit={} cap_mask=0x{:x} hash={}\n",
            id,
            s.state,
            s.exit_code,
            s.cap_mask,
            hex8(&s.source_hash),
        ));
    }
    out
}

fn hex8(b: &[u8; 8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(16);
    for byte in b {
        s.push(H[((*byte >> 4) & 0xF) as usize] as char);
        s.push(H[(*byte & 0xF) as usize] as char);
    }
    s
}

/// Boot smoketest: Rae scripts actually RUN (Concept §Customization Engine
/// — "Swift scripts for automation"). A real script with a loop, a
/// function, and string interpolation executes through the full lifecycle
/// (submit → Completed, exit code + captured output verified); a runaway
/// loop dies by fuel (Timeout, not a hang); a syntax error fails closed;
/// and kill still works on a queued (too-large-for-inline) script.
pub fn run_boot_smoketest() {
    // 1. Real script end-to-end.
    let src = br#"
        func double(n) { return n * 2 }
        var total = 0
        var i = 1
        while i <= 10 { total = total + i
            i = i + 1 }
        print("sum: \(total), doubled: \(double(total))")
        return total
    "#;
    let id = submit(src, 0);
    let st = status(id);
    let ran = st
        .as_ref()
        .map(|s| s.state == State::Completed as u32 && s.exit_code == 55)
        .unwrap_or(false);
    let output_ok = output_of(id).as_deref() == Some("sum: 55, doubled: 110\n");

    // 2. Runaway loop terminates by fuel.
    let runaway = submit(b"while true { }", 0);
    let fuel_ok = status(runaway)
        .map(|s| s.state == State::Timeout as u32)
        .unwrap_or(false);

    // 3. Garbage fails closed.
    let bad = submit(b"let = while", 0);
    let reject_ok = status(bad)
        .map(|s| s.state == State::Failed as u32)
        .unwrap_or(false);

    // 4. Lifecycle: a source too large for inline stays live and can be
    // killed (the userspace-daemon path).
    let big = alloc::vec![b' '; INLINE_MAX_BYTES + 1];
    let queued = submit(&big, 0);
    let kill_ok = kill(queued) == 0
        && status(queued)
            .map(|s| s.state == State::Killed as u32)
            .unwrap_or(false);

    // 5. v0.2 language surface runs IN-KERNEL: closures, arrays, method
    // chains ("RAE-OS".count == 6).
    let v2 = submit(
        br#"
        let names = ["rae", "os"]
        let caps = names.map({ n in n.uppercased() })
        return caps.joined("-").count
    "#,
        0,
    );
    let v2_ok = status(v2)
        .map(|s| s.state == State::Completed as u32 && s.exit_code == 6)
        .unwrap_or(false);

    // 6. Capability-GRANTED binding: a real config_registry roundtrip
    // through the script surface (write, read back, compare).
    let granted = submit(
        br#"
        setConfig("/scripting/smoketest", "ok")
        if getConfig("/scripting/smoketest") == "ok" { return 1 }
        return 0
    "#,
        SCRIPT_CAP_CONFIG,
    );
    let cap_ok = status(granted)
        .map(|s| s.state == State::Completed as u32 && s.exit_code == 1)
        .unwrap_or(false);

    // 7. Capability-DENIED binding fails the script CLOSED (AthGuard:
    // deny by default — cap_mask=0 grants nothing).
    let denied = submit(br#"setConfig("/scripting/smoketest", "no")"#, 0);
    let denied_ok = status(denied)
        .map(|s| s.state == State::Failed as u32)
        .unwrap_or(false)
        && output_of(denied)
            .map(|o| o.contains("CapabilityDenied"))
            .unwrap_or(false);

    // 8. Daemon lifecycle: a queued >64 KiB script is fetchable (Queued →
    // Running, source handed over) and completable — the exact protocol
    // athlangd speaks via SCRIPT_FETCH/SCRIPT_COMPLETE.
    let mut big2 = alloc::vec![b' '; INLINE_MAX_BYTES + 1];
    big2.extend_from_slice(b"return 7");
    let daemon_id = submit(&big2, 0);
    let fetched = fetch_next_queued();
    let daemon_ok = match fetched {
        Some((fid, _caps, src)) if fid == daemon_id && src.len() == big2.len() => {
            complete(fid, 7, b"daemon output") == 0
                && status(fid)
                    .map(|s| s.state == State::Completed as u32 && s.exit_code == 7)
                    .unwrap_or(false)
        }
        _ => false,
    };

    let pass = ran
        && output_ok
        && fuel_ok
        && reject_ok
        && kill_ok
        && v2_ok
        && cap_ok
        && denied_ok
        && daemon_ok;
    crate::serial_println!(
        "[scripting] smoketest: script_ran(exit=55)={} output={} runaway_timeout={} reject_garbage={} kill={} v2_lang(exit=6)={} cap_granted={} cap_denied_closed={} daemon_fetch_complete={} -> {}",
        ran,
        output_ok,
        fuel_ok,
        reject_ok,
        kill_ok,
        v2_ok,
        cap_ok,
        denied_ok,
        daemon_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
