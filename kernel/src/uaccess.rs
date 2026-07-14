//! `uaccess` — the single validated chokepoint for kernel access to USER memory.
//!
//! Concept §"Security by default, not by friction": every syscall that
//! dereferences a user-supplied pointer must first PROVE the pointer names user
//! memory. Otherwise a malicious (or buggy) process hands the kernel a KERNEL
//! address as a "buffer" and the kernel reads or writes kernel memory on the
//! process's behalf — a classic information-leak / privilege-escalation class.
//!
//! The `syscall.rs` (native) and `linux_syscall.rs` (Linux ABI) fast paths
//! already validate via [`crate::extable`]. Several native handlers
//! (`config_registry`, `game_profile`, `app_bundle`) DID bounds-check their
//! pointers (via the `validate_r`/`validate_w` closures the dispatcher passes
//! in), but then did a RAW `copy_nonoverlapping` / `read_unaligned` with NO
//! fault fixup — a TOCTOU robustness hole: a sibling CPU unmapping the validated
//! user page between the check and the copy faults ring 0 with no recovery (and
//! risks a deadlock in the page-fault handler — the exact class the Linux path
//! already hardened). This module is the one surface they route through: a pure
//! bounds check ([`user_range_ok`] — non-null, no wraparound, entirely below the
//! kernel boundary) as defense-in-depth, followed by an extable fault-fixup copy
//! (a raced-unmapped page yields `Err`, never a ring-0 fault). Consolidating here
//! also gives real SMAP one place to wrap `stac`/`clac`.

use alloc::string::String;
use alloc::vec::Vec;

/// First address of the kernel half on x86-64 (four-level paging): every valid
/// user range must END at or below this. Matches the boundary the syscall fast
/// paths use.
pub const USER_ADDR_MAX: u64 = 0x0000_8000_0000_0000;

/// Pure bounds check: is `[ptr, ptr + len)` a well-formed USER range? Rejects a
/// null base (with a non-zero length), an arithmetic wraparound, and any range
/// that reaches into the kernel half. This is exactly the discrimination the raw
/// derefs were missing. Pure + total, so it is trivially auditable and is what
/// the boot smoketest exercises.
#[inline]
pub fn user_range_ok(ptr: u64, len: u64) -> bool {
    if len == 0 {
        return true; // empty range dereferences nothing
    }
    if ptr == 0 {
        return false; // null base with a real length
    }
    match ptr.checked_add(len) {
        Some(end) => end <= USER_ADDR_MAX,
        None => false, // ptr + len overflowed the address space
    }
}

/// Copy `len` bytes FROM a user pointer into a fresh buffer. `Err(())` if the
/// range is not valid user memory, or a page faults mid-copy (mapped to EFAULT
/// by the caller). Never faults ring 0 on a bad pointer.
pub fn copy_from_user(ptr: u64, len: usize) -> Result<Vec<u8>, ()> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if !user_range_ok(ptr, len as u64) {
        return Err(());
    }
    let mut out = Vec::with_capacity(len);
    unsafe {
        out.set_len(len);
        // extable fixup: a TOCTOU-unmapped page rewrites RIP to the fault label
        // and returns Err, rather than a #PF the kernel can't recover in place.
        crate::extable::copy_user_with_fixup(ptr as *const u8, out.as_mut_ptr(), len)?;
    }
    Ok(out)
}

/// Copy `bytes` TO a user pointer. `Err(())` if the destination is not valid
/// user memory, or faults mid-write. This is the write half the raw
/// `copy_nonoverlapping(_, out_ptr, _)` handlers were missing — without it a
/// syscall can be steered to overwrite KERNEL memory.
pub fn copy_to_user(ptr: u64, bytes: &[u8]) -> Result<(), ()> {
    if bytes.is_empty() {
        return Ok(());
    }
    if !user_range_ok(ptr, bytes.len() as u64) {
        return Err(());
    }
    unsafe {
        crate::extable::copy_user_with_fixup(bytes.as_ptr(), ptr as *mut u8, bytes.len())?;
    }
    Ok(())
}

/// Copy `buf.len()` bytes FROM a user pointer into a caller-provided kernel
/// buffer — the NO-ALLOCATION variant of [`copy_from_user`], safe to call from
/// interrupt context (e.g. the page-fault handler's diagnostic user-stack dump,
/// where a heap allocation could deadlock on the allocator lock). Same
/// validation + extable fault-fixup semantics.
pub fn copy_from_user_into(ptr: u64, buf: &mut [u8]) -> Result<(), ()> {
    if buf.is_empty() {
        return Ok(());
    }
    if !user_range_ok(ptr, buf.len() as u64) {
        return Err(());
    }
    unsafe {
        crate::extable::copy_user_with_fixup(ptr as *const u8, buf.as_mut_ptr(), buf.len())?;
    }
    Ok(())
}

/// Read the bytes of a NUL-terminated user string, bounded by `max_len`.
/// Replaces the byte-at-a-time raw scan (`syscall::read_user_cstr`) that
/// dereferenced the user pointer directly: this walks the string in page-sized
/// chunks, each chunk copied through the validated extable chokepoint, and
/// scans the KERNEL-side copy for the terminator. Semantics match the scanner
/// it replaces: the NUL is not included; if no NUL appears within `max_len`
/// bytes the truncated prefix is returned (NOT an error). `Err(())` only if
/// the base is not user memory or a page is unmapped/unreadable mid-scan.
pub fn read_user_cstr_bytes(ptr: u64, max_len: usize) -> Result<Vec<u8>, ()> {
    if max_len == 0 {
        return Ok(Vec::new());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut addr = ptr;
    let mut remaining = max_len;
    let mut chunk = [0u8; 256];
    while remaining > 0 {
        // Stay inside one page per copy so a fault on a later page doesn't
        // discard the bytes already read (mirrors the old per-page revalidate).
        let to_page_end = 0x1000 - (addr as usize & 0xFFF);
        let n = remaining.min(chunk.len()).min(to_page_end);
        copy_from_user_into(addr, &mut chunk[..n])?;
        if let Some(nul) = chunk[..n].iter().position(|&b| b == 0) {
            out.extend_from_slice(&chunk[..nul]);
            return Ok(out);
        }
        out.extend_from_slice(&chunk[..n]);
        addr = addr.checked_add(n as u64).ok_or(())?;
        remaining -= n;
    }
    Ok(out) // no NUL within max_len — truncated, like the scanner it replaces
}

/// Validated drop-in for the old unsafe `read_user_bytes(ptr, len) -> Vec<u8>`
/// helpers: an empty `Vec` on any validation/copy failure (matches the prior
/// call-site behaviour, minus the arbitrary-kernel-read hole).
pub fn read_user_bytes(ptr: u64, len: u64) -> Vec<u8> {
    copy_from_user(ptr, len as usize).unwrap_or_default()
}

/// Validated drop-in for the old unsafe `read_user_string(ptr, len) -> String`:
/// a fixed-length, lossy-UTF8 read; empty on any failure.
pub fn read_user_string(ptr: u64, len: u64) -> String {
    String::from_utf8(read_user_bytes(ptr, len)).unwrap_or_default()
}

/// R10 FAIL-able boot smoketest: the bounds gate must REJECT a kernel pointer, a
/// boundary-straddling range, a wraparound, and a null base, ACCEPT a normal
/// user range, and `copy_from_user` on a kernel pointer must return `Err`
/// WITHOUT faulting (the gate short-circuits before any dereference). A
/// regression that widens the gate flips a `true` to `false` and prints FAIL.
pub fn run_boot_smoketest() {
    let kernel_ptr = 0xFFFF_8000_0000_0000u64; // canonical kernel-half address
    let user_ok = user_range_ok(0x1000, 0x2000); // a plausible user range
    let rej_kernel = !user_range_ok(kernel_ptr, 16);
    let rej_boundary = !user_range_ok(USER_ADDR_MAX - 8, 16); // straddles the line
    let rej_wrap = !user_range_ok(u64::MAX - 4, 64); // ptr + len wraps
    let rej_null = !user_range_ok(0, 16);
    // End-to-end: reject a kernel pointer before any deref (no ring-0 fault).
    let cfu_rejects_kernel = copy_from_user(kernel_ptr, 16).is_err();
    let pass = user_ok && rej_kernel && rej_boundary && rej_wrap && rej_null && cfu_rejects_kernel;
    crate::serial_println!(
        "[uaccess] run_boot_smoketest: user_ok={} rej(kernel={} boundary={} wrap={} null={}) cfu_rejects_kernel={} -> {}",
        user_ok,
        rej_kernel,
        rej_boundary,
        rej_wrap,
        rej_null,
        cfu_rejects_kernel,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// procfs `/proc/raeen/uaccess`: the hardened-chokepoint status line.
pub fn procfs_status() -> String {
    alloc::format!(
        "uaccess: user_addr_max={:#018x} chokepoint=copy_from_user/copy_to_user \
         backing=extable-fixup validated=bounds+pagewalk\n",
        USER_ADDR_MAX
    )
}
