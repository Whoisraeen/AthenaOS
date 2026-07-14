//! Thin userspace ABI over `perm_prompt`.
//!
//! The permission-prompt queue is the kernel-side machinery behind AthenaOS's
//! capability-based permissions (Concept §Security: "Apps request
//! capabilities, user grants, OS enforces at the syscall layer"). The
//! Settings → Privacy panel needs to enumerate pending requests, show a
//! user-friendly description, and call back with the verdict — none of
//! which was wired as syscalls.
//!
//! This module exposes three calls and nothing else; the policy lives in
//! `perm_prompt.rs`.
//!
//! ## Syscalls (71-73)
//!
//! | nr | name              | rdi/rsi/rdx                                            | rax |
//! |----|-------------------|--------------------------------------------------------|----|
//! | 71 | PERM_LIST         | rdi=out_ptr, rsi=out_cap_bytes (PermAbi entries)       | count |
//! | 72 | PERM_RESPOND      | rdi=request_id, rsi=approved(0/1)                      | 0/err |
//! | 73 | PERM_STATS        | rdi=out_ptr (u64×2 = pending, total_resolved_ish)      | 16 |

#![allow(dead_code)]

extern crate alloc;

use crate::perm_prompt;
use alloc::vec::Vec;

/// 80-byte fixed ABI struct describing one pending permission request.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PermAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub requester: u64, // task id
    pub flavor: u32,    // CapFlavor as u32
    pub rights: u32,    // RightsFlags raw bits
    pub created_tick: u64,
    pub app_name: [u8; 24],
    pub description: [u8; 24],
}

const PERM_ABI_SIZE: usize = core::mem::size_of::<PermAbi>();

pub const SYS_PERM_LIST: u64 = 71;
pub const SYS_PERM_RESPOND: u64 = 72;
pub const SYS_PERM_STATS: u64 = 73;

/// True when the CALLING task holds a `Cap::System` covering `needed`.
/// The prompt queue is trusted-shell-only surface (module docstring): without
/// this gate any task could list other apps' pending requests (info leak) or
/// approve ITS OWN request (full sandbox escape — the resolve path now
/// performs a real `insert_root` grant). user_init (the trusted shell root)
/// is seeded a System cap at spawn; it derives narrower ones to Settings.
fn caller_has_system(needed: crate::capability::Rights) -> bool {
    crate::scheduler::with_current_task(|t| {
        t.cap_table.iter().any(|(_, cap)| {
            matches!(cap, crate::capability::Cap::System { rights } if rights.contains(needed))
        })
    })
    .unwrap_or(false)
}

pub fn sys_perm_list(
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    // Trusted-shell only: pending requests describe OTHER apps' intents.
    if !caller_has_system(crate::capability::Rights::READ) {
        return 0;
    }
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let max = (out_cap as usize) / PERM_ABI_SIZE;
    if max == 0 {
        return 0;
    }

    let pending: Vec<perm_prompt::PermRequest> = perm_prompt::drain_pending(max);
    let n = pending.len();

    // SMAP-safe: serialize each PermRequest into a kernel buffer matching the
    // repr(C) PermAbi layout (version, 4B pad, id@8, requester@16, flavor@24,
    // rights@28, created_tick@32, app_name@40, description@64) and do one
    // validated extable copy-out. Manual packing avoids padding-byte exposure
    // and the raw user-ptr deref.
    debug_assert_eq!(PERM_ABI_SIZE, 88);
    let mut out: Vec<u8> = Vec::with_capacity(n * PERM_ABI_SIZE);
    for req in pending.iter() {
        let mut e = [0u8; 88];
        let a = req.app_name.as_bytes();
        let d = req.description.as_bytes();
        e[0..4].copy_from_slice(&1u32.to_le_bytes()); // version
        e[8..16].copy_from_slice(&req.id.0.to_le_bytes());
        e[16..24].copy_from_slice(&req.requester.raw().to_le_bytes());
        e[24..28].copy_from_slice(&(req.flavor as u32).to_le_bytes());
        e[28..32].copy_from_slice(&req.rights.bits().to_le_bytes());
        e[32..40].copy_from_slice(&req.created_tick.to_le_bytes());
        e[40..40 + a.len().min(24)].copy_from_slice(&a[..a.len().min(24)]);
        e[64..64 + d.len().min(24)].copy_from_slice(&d[..d.len().min(24)]);
        out.extend_from_slice(&e);
    }
    if crate::uaccess::copy_to_user(out_ptr, &out).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_perm_respond(request_id: u64, approved: u64) -> u64 {
    // WRITE-level System cap required: respond performs a REAL capability
    // grant into the requester's table. Without this gate any sandboxed task
    // could approve its own pending request — a one-syscall sandbox escape.
    if !caller_has_system(crate::capability::Rights::WRITE) {
        return u64::MAX;
    }
    perm_prompt::resolve(perm_prompt::RequestId(request_id), approved != 0);
    0
}

pub fn sys_perm_stats(
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if out_cap < 16 {
        return 0;
    }
    if !validate_w(out_ptr, 16, true) {
        return 0;
    }
    let pending = perm_prompt::pending_count() as u64;
    // SMAP-safe: kernel-side pack + one validated extable copy-out.
    let mut buf = [0u8; 16];
    buf[0..8].copy_from_slice(&pending.to_le_bytes());
    // buf[8..16] reserved (zero)
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    16
}

// ── /proc/athena/perm ───────────────────────────────────────────────────

pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    let n = perm_prompt::pending_count();
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS permission-prompt queue ({} pending)\n",
        n,
    ));
    // drain_pending here would consume; we want a peek instead — read
    // count and provide guidance. Trusted Settings UI uses SYS_PERM_LIST
    // for the real values.
    out.push_str("# (peek only — use SYS_PERM_LIST/RESPOND to act)\n");
    out
}
