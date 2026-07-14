//! x64 structured-exception-handling (SEH) engine — Concept §Compatibility
//! Strategy: "AthBridge runs Windows apps on day one."
//!
//! Unlike x86 (which threads exception frames through a `FS:[0]` linked list),
//! x86-64 Windows uses **table-based** exception handling. Every non-leaf
//! function ships a `RUNTIME_FUNCTION` in `.pdata` and an `UNWIND_INFO` blob in
//! `.xdata` describing exactly how its prolog established the frame. The OS
//! exception dispatcher (`RtlDispatchException`) and the unwinder
//! (`RtlVirtualUnwind`) read those tables to walk the stack, restore
//! nonvolatile registers, and find the language handler that owns a fault.
//!
//! This is load-bearing for real Windows code: the MSVC CRT installs unwind
//! info for `_start`/`mainCRTStartup`; C++ `throw`/`catch`, `__try`/`__except`
//! (used pervasively by anti-cheat, anti-debug, and structured cleanup), and
//! every crash reporter's stack walk all go through this machinery. Without a
//! correct table-walking engine, the first AV/divide-by-zero/`int 3` in a real
//! `.exe` unwinds into garbage instead of its handler.
//!
//! This module is the **pure-logic engine** (MasterChecklist Phase 11.2: "SEH
//! (structured exception handling) translation"):
//!   * [`parse_pdata`] / [`lookup_function`] — the `.pdata` `RUNTIME_FUNCTION`
//!     table + binary search by RIP.
//!   * [`parse_unwind_info`] — the `.xdata` `UNWIND_INFO` header + unwind codes
//!     + chained-info / language-handler tail.
//!   * [`virtual_unwind`] — fold one logical frame's prolog back: restore
//!     nonvolatile GPRs, follow `UNW_FLAG_CHAININFO`, pop the return address.
//!   * [`dispatch_exception`] — walk the call chain from a fault, honoring
//!     `UNW_FLAG_EHANDLER`, and (for `__C_specific_handler`) locate the matching
//!     `SCOPE_TABLE` record.
//!
//! Delivering a *live* hardware fault into a guest handler needs kernel signal
//! plumbing (a later slice); the table engine — the part that has to be exactly
//! right or every dispatch is wrong — is proven here off-target with host KATs
//! over hand-built `.pdata`/`.xdata`.

extern crate alloc;

use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// PE / unwind constants
// ---------------------------------------------------------------------------

/// `IMAGE_DIRECTORY_ENTRY_EXCEPTION` — the data-directory index of `.pdata`.
pub const IMAGE_DIRECTORY_ENTRY_EXCEPTION: usize = 3;

/// `UNW_FLAG_EHANDLER` — the function has a language exception handler.
pub const UNW_FLAG_EHANDLER: u8 = 0x01;
/// `UNW_FLAG_UHANDLER` — the function has a termination (unwind) handler.
pub const UNW_FLAG_UHANDLER: u8 = 0x02;
/// `UNW_FLAG_CHAININFO` — the tail is a chained `RUNTIME_FUNCTION`, not a handler.
pub const UNW_FLAG_CHAININFO: u8 = 0x04;

// x64 unwind operation codes (`UNWIND_CODE.UnwindOp`).
const UWOP_PUSH_NONVOL: u8 = 0;
const UWOP_ALLOC_LARGE: u8 = 1;
const UWOP_ALLOC_SMALL: u8 = 2;
const UWOP_SET_FPREG: u8 = 3;
const UWOP_SAVE_NONVOL: u8 = 4;
const UWOP_SAVE_NONVOL_FAR: u8 = 5;
const UWOP_SAVE_XMM128: u8 = 8;
const UWOP_SAVE_XMM128_FAR: u8 = 9;
const UWOP_PUSH_MACHFRAME: u8 = 10;

// x64 integer register numbering, as used by `OpInfo` in the unwind codes.
pub const REG_RAX: usize = 0;
pub const REG_RCX: usize = 1;
pub const REG_RDX: usize = 2;
pub const REG_RBX: usize = 3;
pub const REG_RSP: usize = 4;
pub const REG_RBP: usize = 5;
pub const REG_RSI: usize = 6;
pub const REG_RDI: usize = 7;
pub const REG_R8: usize = 8;
pub const REG_R12: usize = 12;
pub const REG_R15: usize = 15;

// ---------------------------------------------------------------------------
// Register context
// ---------------------------------------------------------------------------

/// The subset of a thread context the unwinder reads and rewrites: the
/// instruction pointer plus the 16 integer registers (indexed by the x64
/// register number, so `gpr[REG_RSP]` is RSP). XMM state isn't modeled — the
/// unwinder tracks `SAVE_XMM128` codes for slot accounting but GPR unwinding,
/// the stack pointer, and the return address are what frame-walking needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegContext {
    pub rip: u64,
    pub gpr: [u64; 16],
}

impl RegContext {
    /// Current stack pointer.
    #[inline]
    pub fn rsp(&self) -> u64 {
        self.gpr[REG_RSP]
    }
    /// Set the stack pointer.
    #[inline]
    pub fn set_rsp(&mut self, v: u64) {
        self.gpr[REG_RSP] = v;
    }
    /// Current frame pointer (RBP).
    #[inline]
    pub fn rbp(&self) -> u64 {
        self.gpr[REG_RBP]
    }
}

// ---------------------------------------------------------------------------
// Process-memory abstraction
// ---------------------------------------------------------------------------

/// Reads 8 bytes of the running process's memory at an absolute virtual
/// address. The unwinder uses it to fetch saved nonvolatile registers off the
/// stack and to pop return addresses. In the AthBridge host process guest VAs
/// are host VAs, so the production impl is a checked raw load; host KATs back it
/// with [`SliceMemory`] over a `Vec<u8>`. Returns `None` for an unreadable
/// address (a corrupt or smashed stack must fail the walk, never fault it).
pub trait MemoryReader {
    fn read_u64(&self, addr: u64) -> Option<u64>;
}

/// A flat slice of memory mapped at `base`, implementing [`MemoryReader`]. Used
/// by [`run_self_test`] and the host KATs to model a guest stack without any
/// real process memory.
pub struct SliceMemory<'a> {
    pub base: u64,
    pub bytes: &'a [u8],
}

impl MemoryReader for SliceMemory<'_> {
    fn read_u64(&self, addr: u64) -> Option<u64> {
        let off = addr.checked_sub(self.base)? as usize;
        let end = off.checked_add(8)?;
        if end > self.bytes.len() {
            return None;
        }
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.bytes[off..end]);
        Some(u64::from_le_bytes(b))
    }
}

// ---------------------------------------------------------------------------
// Bounds-checked little-endian readers over the mapped image (RVA-indexed)
// ---------------------------------------------------------------------------
//
// `image` is the loaded PE where byte offset == RVA. Every read is checked: the
// `.pdata`/`.xdata`/scope-table bytes come from an untrusted file, so a hostile
// or truncated table must yield `None`, never an out-of-bounds panic.

#[inline]
fn img_u8(image: &[u8], rva: u32) -> Option<u8> {
    image.get(rva as usize).copied()
}

#[inline]
fn img_u16(image: &[u8], rva: u32) -> Option<u16> {
    let i = rva as usize;
    let s = image.get(i..i + 2)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
fn img_u32(image: &[u8], rva: u32) -> Option<u32> {
    let i = rva as usize;
    let s = image.get(i..i + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

// ---------------------------------------------------------------------------
// .pdata — RUNTIME_FUNCTION table
// ---------------------------------------------------------------------------

/// One `RUNTIME_FUNCTION` (`.pdata` entry, 12 bytes): the half-open RVA range
/// `[begin, end)` the record covers and the RVA of its `UNWIND_INFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeFunction {
    pub begin: u32,
    pub end: u32,
    pub unwind_info: u32,
}

impl RuntimeFunction {
    /// Does this record cover image-relative address `rva`?
    #[inline]
    pub fn covers(&self, rva: u32) -> bool {
        rva >= self.begin && rva < self.end
    }
}

/// Parse the `.pdata` exception directory into its `RUNTIME_FUNCTION` array.
/// `pdata_rva`/`pdata_size` come from `data_directories[IMAGE_DIRECTORY_ENTRY_EXCEPTION]`.
pub fn parse_pdata(image: &[u8], pdata_rva: u32, pdata_size: u32) -> Vec<RuntimeFunction> {
    let mut out = Vec::new();
    let count = (pdata_size / 12) as u32;
    for i in 0..count {
        let off = pdata_rva + i * 12;
        let (begin, end, unwind_info) = match (
            img_u32(image, off),
            img_u32(image, off + 4),
            img_u32(image, off + 8),
        ) {
            (Some(b), Some(e), Some(u)) => (b, e, u),
            _ => break,
        };
        // A zeroed entry terminates the table early (some linkers pad).
        if begin == 0 && end == 0 && unwind_info == 0 {
            break;
        }
        out.push(RuntimeFunction {
            begin,
            end,
            unwind_info,
        });
    }
    out
}

/// Find the `RUNTIME_FUNCTION` covering image-relative `rva`. `.pdata` is sorted
/// ascending by `begin`, so this binary-searches; it tolerates an unsorted table
/// by falling back to a linear scan (a malformed `.pdata` must still resolve
/// correctly, not silently miss a frame).
pub fn lookup_function(funcs: &[RuntimeFunction], rva: u32) -> Option<RuntimeFunction> {
    if funcs.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (0usize, funcs.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let f = funcs[mid];
        if rva < f.begin {
            hi = mid;
        } else if rva >= f.end {
            lo = mid + 1;
        } else {
            return Some(f);
        }
    }
    // Sorted search missed — tolerate an unsorted table with a linear pass.
    funcs.iter().copied().find(|f| f.covers(rva))
}

// ---------------------------------------------------------------------------
// .xdata — UNWIND_INFO
// ---------------------------------------------------------------------------

/// Parsed `UNWIND_INFO`: the prolog description for one function (or one link of
/// a chained one). `nodes` holds the raw little-endian `UNWIND_CODE` array —
/// multi-node codes (alloc-large, save-nonvol) read their operands from the
/// following nodes, so keeping them raw is both simplest and exactly faithful to
/// the on-disk layout.
#[derive(Clone, Debug)]
pub struct UnwindInfo {
    pub version: u8,
    pub flags: u8,
    pub size_of_prolog: u8,
    /// Frame register number (0 = none established).
    pub frame_register: u8,
    /// Frame-pointer offset in 16-byte units (`SET_FPREG` scale).
    pub frame_register_offset: u8,
    pub nodes: Vec<u16>,
    /// `UNW_FLAG_CHAININFO`: `chained` holds the parent `RUNTIME_FUNCTION`.
    pub chained: Option<RuntimeFunction>,
    /// `UNW_FLAG_EHANDLER|UHANDLER`: RVA of the language-specific handler.
    pub handler_rva: Option<u32>,
    /// RVA of the language-specific handler data (e.g. the `__C_specific_handler`
    /// `SCOPE_TABLE`), immediately after the handler RVA.
    pub handler_data_rva: u32,
}

impl UnwindInfo {
    /// Does the function have an *exception* (search-phase) handler?
    #[inline]
    pub fn has_exception_handler(&self) -> bool {
        self.flags & UNW_FLAG_EHANDLER != 0
    }
    /// Does the function have a termination (unwind-phase) handler?
    #[inline]
    pub fn has_termination_handler(&self) -> bool {
        self.flags & UNW_FLAG_UHANDLER != 0
    }
}

/// Slot count (in 2-byte nodes, including the op node itself) one unwind code
/// consumes. Driven purely by `(op, op_info)` per the AMD64 unwind spec.
fn node_count(op: u8, op_info: u8) -> usize {
    match op {
        UWOP_ALLOC_LARGE => {
            if op_info == 0 {
                2
            } else {
                3
            }
        }
        UWOP_SAVE_NONVOL | UWOP_SAVE_XMM128 => 2,
        UWOP_SAVE_NONVOL_FAR | UWOP_SAVE_XMM128_FAR => 3,
        // PUSH_NONVOL, ALLOC_SMALL, SET_FPREG, PUSH_MACHFRAME, and the spare
        // codes (6/7) are single-node.
        _ => 1,
    }
}

#[inline]
fn code_offset(node: u16) -> u8 {
    (node & 0xFF) as u8
}
#[inline]
fn code_op(node: u16) -> u8 {
    ((node >> 8) & 0x0F) as u8
}
#[inline]
fn code_op_info(node: u16) -> u8 {
    ((node >> 12) & 0x0F) as u8
}

/// Parse the `UNWIND_INFO` at image-relative `rva`. Returns `None` on any
/// truncation or an unsupported version (only the v1 layout exists in the wild).
pub fn parse_unwind_info(image: &[u8], rva: u32) -> Option<UnwindInfo> {
    let b0 = img_u8(image, rva)?;
    let version = b0 & 0x07;
    let flags = b0 >> 3;
    if version != 1 {
        return None;
    }
    let size_of_prolog = img_u8(image, rva + 1)?;
    let count = img_u8(image, rva + 2)?;
    let fr_byte = img_u8(image, rva + 3)?;
    let frame_register = fr_byte & 0x0F;
    let frame_register_offset = fr_byte >> 4;

    let mut nodes = Vec::with_capacity(count as usize);
    for i in 0..count as u32 {
        nodes.push(img_u16(image, rva + 4 + i * 2)?);
    }

    // The code array is padded to an even node count so the tail (chain record
    // or handler RVA) is 4-byte aligned relative to the 4-byte header.
    let padded_nodes = ((count as u32) + 1) & !1;
    let tail_rva = rva + 4 + padded_nodes * 2;

    let mut chained = None;
    let mut handler_rva = None;
    let mut handler_data_rva = 0;

    if flags & UNW_FLAG_CHAININFO != 0 {
        let begin = img_u32(image, tail_rva)?;
        let end = img_u32(image, tail_rva + 4)?;
        let ui = img_u32(image, tail_rva + 8)?;
        chained = Some(RuntimeFunction {
            begin,
            end,
            unwind_info: ui,
        });
    } else if flags & (UNW_FLAG_EHANDLER | UNW_FLAG_UHANDLER) != 0 {
        handler_rva = Some(img_u32(image, tail_rva)?);
        handler_data_rva = tail_rva + 4;
    }

    Some(UnwindInfo {
        version,
        flags,
        size_of_prolog,
        frame_register,
        frame_register_offset,
        nodes,
        chained,
        handler_rva,
        handler_data_rva,
    })
}

// ---------------------------------------------------------------------------
// __C_specific_handler SCOPE_TABLE
// ---------------------------------------------------------------------------

/// One `SCOPE_TABLE` record (`__C_specific_handler` language data, 16 bytes).
/// All four fields are image RVAs. `handler == 1` marks a `__finally`; otherwise
/// `handler` is the `__except` filter funclet and `target` is where execution
/// resumes when the filter returns `EXCEPTION_EXECUTE_HANDLER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeRecord {
    pub begin: u32,
    pub end: u32,
    pub handler: u32,
    pub target: u32,
}

impl ScopeRecord {
    #[inline]
    pub fn covers(&self, rva: u32) -> bool {
        rva >= self.begin && rva < self.end
    }
    /// `__finally` blocks encode `handler == 1`.
    #[inline]
    pub fn is_finally(&self) -> bool {
        self.handler == 1
    }
}

/// Parse the `SCOPE_TABLE` at `data_rva` (a `u32` count followed by that many
/// 16-byte records).
pub fn parse_scope_table(image: &[u8], data_rva: u32) -> Vec<ScopeRecord> {
    let mut out = Vec::new();
    let count = match img_u32(image, data_rva) {
        Some(c) => c,
        None => return out,
    };
    // Cap to keep a corrupt count from spinning; no real function has this many.
    let count = count.min(4096);
    for i in 0..count {
        let off = data_rva + 4 + i * 16;
        match (
            img_u32(image, off),
            img_u32(image, off + 4),
            img_u32(image, off + 8),
            img_u32(image, off + 12),
        ) {
            (Some(b), Some(e), Some(h), Some(t)) => out.push(ScopeRecord {
                begin: b,
                end: e,
                handler: h,
                target: t,
            }),
            _ => break,
        }
    }
    out
}

/// Find the innermost `SCOPE_TABLE` record whose guarded region covers `rva`.
/// MSVC emits inner scopes before the scopes that enclose them, so the first
/// match is the innermost — exactly the dispatch order `__C_specific_handler`
/// itself uses.
pub fn find_scope(image: &[u8], data_rva: u32, rva: u32) -> Option<ScopeRecord> {
    parse_scope_table(image, data_rva)
        .into_iter()
        .find(|r| r.covers(rva))
}

// ---------------------------------------------------------------------------
// Virtual unwind — fold one logical frame back to its caller
// ---------------------------------------------------------------------------

/// Why a virtual unwind couldn't complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnwindError {
    /// The `UNWIND_INFO` was truncated or an unsupported version.
    BadUnwindInfo,
    /// A required stack slot (saved register or return address) was unreadable.
    BadStackRead,
    /// A chained-info loop exceeded the sane link bound (corrupt `.xdata`).
    ChainTooDeep,
}

/// Apply the unwind codes of a single `UNWIND_INFO` node, folding `ctx` back
/// over that node's prolog. `prolog_offset` is how many bytes into the function
/// the active RIP sits (`u64::MAX` for "prolog fully executed"); codes whose
/// `CodeOffset` is *beyond* it haven't run yet and are skipped. Returns `true`
/// if a machine frame (`UWOP_PUSH_MACHFRAME`) set RIP/RSP directly, so the
/// caller must not also pop a return address.
fn apply_unwind_codes(
    ui: &UnwindInfo,
    ctx: &mut RegContext,
    frame_base: u64,
    prolog_offset: u64,
    reader: &dyn MemoryReader,
) -> Result<bool, UnwindError> {
    let nodes = &ui.nodes;
    let mut i = 0usize;
    while i < nodes.len() {
        let node = nodes[i];
        let op = code_op(node);
        let op_info = code_op_info(node);
        let off = code_offset(node) as u64;
        let count = node_count(op, op_info);

        // Skip operations whose prolog instruction hasn't executed yet (fault
        // landed mid-prolog). They still consume their nodes.
        if off > prolog_offset {
            i += count;
            continue;
        }

        match op {
            UWOP_PUSH_NONVOL => {
                let v = reader
                    .read_u64(ctx.gpr[REG_RSP])
                    .ok_or(UnwindError::BadStackRead)?;
                ctx.gpr[op_info as usize] = v;
                ctx.gpr[REG_RSP] = ctx.gpr[REG_RSP].wrapping_add(8);
            }
            UWOP_ALLOC_LARGE => {
                let size = if op_info == 0 {
                    // size / 8 in the next node.
                    (*nodes.get(i + 1).ok_or(UnwindError::BadUnwindInfo)? as u64) * 8
                } else {
                    // full byte size in the next two nodes (low, high).
                    let lo = *nodes.get(i + 1).ok_or(UnwindError::BadUnwindInfo)? as u64;
                    let hi = *nodes.get(i + 2).ok_or(UnwindError::BadUnwindInfo)? as u64;
                    lo | (hi << 16)
                };
                ctx.gpr[REG_RSP] = ctx.gpr[REG_RSP].wrapping_add(size);
            }
            UWOP_ALLOC_SMALL => {
                ctx.gpr[REG_RSP] = ctx.gpr[REG_RSP].wrapping_add((op_info as u64) * 8 + 8);
            }
            UWOP_SET_FPREG => {
                let fp = ctx.gpr[ui.frame_register as usize];
                ctx.gpr[REG_RSP] = fp.wrapping_sub((ui.frame_register_offset as u64) * 16);
            }
            UWOP_SAVE_NONVOL => {
                let scaled = *nodes.get(i + 1).ok_or(UnwindError::BadUnwindInfo)? as u64;
                let addr = frame_base.wrapping_add(scaled * 8);
                let v = reader.read_u64(addr).ok_or(UnwindError::BadStackRead)?;
                ctx.gpr[op_info as usize] = v;
            }
            UWOP_SAVE_NONVOL_FAR => {
                let lo = *nodes.get(i + 1).ok_or(UnwindError::BadUnwindInfo)? as u64;
                let hi = *nodes.get(i + 2).ok_or(UnwindError::BadUnwindInfo)? as u64;
                let addr = frame_base.wrapping_add(lo | (hi << 16));
                let v = reader.read_u64(addr).ok_or(UnwindError::BadStackRead)?;
                ctx.gpr[op_info as usize] = v;
            }
            UWOP_SAVE_XMM128 | UWOP_SAVE_XMM128_FAR => {
                // XMM nonvolatiles aren't modeled in the GPR context; the slots
                // are still consumed (handled by `count`) so RSP folding stays
                // aligned with the on-disk code stream.
            }
            UWOP_PUSH_MACHFRAME => {
                // A hardware trap frame: {RIP, CS, EFLAGS, OldRsp, SS}, with an
                // extra error-code qword below it when op_info == 1.
                let base = ctx.gpr[REG_RSP];
                let (rip_off, rsp_off) = if op_info == 0 {
                    (0u64, 0x18u64)
                } else {
                    (8u64, 0x20u64)
                };
                ctx.rip = reader
                    .read_u64(base.wrapping_add(rip_off))
                    .ok_or(UnwindError::BadStackRead)?;
                ctx.gpr[REG_RSP] = reader
                    .read_u64(base.wrapping_add(rsp_off))
                    .ok_or(UnwindError::BadStackRead)?;
                return Ok(true);
            }
            _ => {
                // Spare/unknown opcodes: consume nodes, change nothing.
            }
        }

        i += count;
    }
    Ok(false)
}

/// Maximum `UNW_FLAG_CHAININFO` links followed before declaring the `.xdata`
/// corrupt. Real chains are 1–2 deep; this only stops a malicious loop.
const MAX_CHAIN_LINKS: usize = 32;

/// Virtually unwind one *logical* frame: from a context whose RIP lies in
/// `func`, restore the caller's nonvolatile registers and stack pointer and set
/// `ctx.rip` to the return address. Follows `UNW_FLAG_CHAININFO` so a function
/// whose prolog spans several `UNWIND_INFO`s is fully folded. After this call
/// `ctx` describes the *caller's* frame.
pub fn virtual_unwind(
    image: &[u8],
    image_base: u64,
    ctx: &mut RegContext,
    func: &RuntimeFunction,
    reader: &dyn MemoryReader,
) -> Result<(), UnwindError> {
    let mut rf = *func;
    let mut first = true;
    for _ in 0..MAX_CHAIN_LINKS {
        let ui = parse_unwind_info(image, rf.unwind_info).ok_or(UnwindError::BadUnwindInfo)?;

        // Prolog progress is measured against the *primary* function only; once
        // we've stepped into a chained parent we're unwinding from its body, so
        // its whole prolog has executed.
        let prolog_offset = if first {
            let func_va = image_base.wrapping_add(rf.begin as u64);
            ctx.rip.wrapping_sub(func_va)
        } else {
            u64::MAX
        };

        // SAVE_NONVOL/SAVE_XMM offsets are relative to the frame base: the
        // established frame pointer when one exists, else the entry RSP (the
        // current RSP, since no prolog code has folded it yet this node).
        let frame_base = if ui.frame_register != 0 {
            ctx.gpr[ui.frame_register as usize].wrapping_sub((ui.frame_register_offset as u64) * 16)
        } else {
            ctx.gpr[REG_RSP]
        };

        let machine_frame = apply_unwind_codes(&ui, ctx, frame_base, prolog_offset, reader)?;
        if machine_frame {
            return Ok(());
        }

        match ui.chained {
            Some(parent) => {
                rf = parent;
                first = false;
            }
            None => {
                // Pop the return address: it sits at [RSP] once the prolog is
                // fully folded.
                let ret = reader
                    .read_u64(ctx.gpr[REG_RSP])
                    .ok_or(UnwindError::BadStackRead)?;
                ctx.rip = ret;
                ctx.gpr[REG_RSP] = ctx.gpr[REG_RSP].wrapping_add(8);
                return Ok(());
            }
        }
    }
    Err(UnwindError::ChainTooDeep)
}

// ---------------------------------------------------------------------------
// Exception dispatch — the search phase
// ---------------------------------------------------------------------------

/// Outcome of walking the call chain from a fault (the `RtlDispatchException`
/// search phase).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// A frame with a language exception handler covers the fault.
    HandlerFound {
        /// RIP of the handling frame (the faulting instruction or a return
        /// address en route).
        frame_rip: u64,
        /// RSP at the handling frame.
        frame_rsp: u64,
        /// RVA of the language handler (e.g. `__C_specific_handler`).
        handler_rva: u32,
        /// For `__C_specific_handler`, the matching scope record (if any).
        scope: Option<ScopeRecord>,
    },
    /// The whole chain was walked with no handler — an unhandled exception
    /// (`UnhandledExceptionFilter` / process crash).
    Unhandled,
    /// A frame's code had no `.pdata` entry (a leaf or non-PE region); the walk
    /// can't continue reliably.
    NoUnwindInfo { rip: u64 },
    /// The unwind tables were corrupt or a stack read failed mid-walk.
    Corrupt { rip: u64 },
}

/// Frame budget for [`dispatch_exception`]. A real call chain that deep is
/// already pathological; this bounds a corrupt-stack loop.
const MAX_DISPATCH_FRAMES: usize = 256;

/// Walk the call chain from the faulting context, honoring `UNW_FLAG_EHANDLER`,
/// and return the disposition. `funcs` is the parsed `.pdata`; `image` is the
/// mapped PE (offset == RVA); `reader` reads the live stack. This is the search
/// phase: it *locates* the handler (and, for `__C_specific_handler`, the scope
/// record) but does not run filter funclets — executing the handler is a
/// separate phase that needs the live process.
pub fn dispatch_exception(
    image: &[u8],
    image_base: u64,
    funcs: &[RuntimeFunction],
    reader: &dyn MemoryReader,
    fault: &RegContext,
) -> Disposition {
    let mut ctx = *fault;
    for _ in 0..MAX_DISPATCH_FRAMES {
        let rva = ctx.rip.wrapping_sub(image_base) as u32;
        let rf = match lookup_function(funcs, rva) {
            Some(rf) => rf,
            None => return Disposition::NoUnwindInfo { rip: ctx.rip },
        };
        let ui = match parse_unwind_info(image, rf.unwind_info) {
            Some(ui) => ui,
            None => return Disposition::Corrupt { rip: ctx.rip },
        };

        if ui.has_exception_handler() {
            let scope = if ui.handler_data_rva != 0 {
                find_scope(image, ui.handler_data_rva, rva)
            } else {
                None
            };
            return Disposition::HandlerFound {
                frame_rip: ctx.rip,
                frame_rsp: ctx.gpr[REG_RSP],
                handler_rva: ui.handler_rva.unwrap_or(0),
                scope,
            };
        }

        // No handler in this frame — unwind to the caller and keep searching.
        match virtual_unwind(image, image_base, &mut ctx, &rf, reader) {
            Ok(()) => {}
            Err(_) => return Disposition::Corrupt { rip: ctx.rip },
        }
        if ctx.rip == 0 {
            return Disposition::Unhandled;
        }
    }
    Disposition::Unhandled
}

// ---------------------------------------------------------------------------
// R10 boot smoketest
// ---------------------------------------------------------------------------

/// Self-test (callable from a kernel R10 boot smoketest). Builds a synthetic
/// `.xdata`/`.pdata` for a function with the canonical prolog
/// `push rbp; push rbx; sub rsp, 0x20`, lays out a matching guest stack in a
/// byte buffer, virtually unwinds one frame, and confirms the restored RBP/RBX,
/// return address, and folded RSP are exactly right. Also exercises the
/// `__C_specific_handler` scope-table search. Returns `true` on PASS; any wrong
/// value returns `false` (a smoketest that cannot FAIL is a false green).
pub fn run_self_test() -> bool {
    const IMAGE_BASE: u64 = 0x1_4000_0000;
    const FUNC_RVA: u32 = 0x1000;
    const UNWIND_RVA: u32 = 0x2000;
    const SCOPE_RVA: u32 = 0x2040;

    // Build the mapped image: just the .xdata we need, indexed by RVA.
    let mut image = alloc::vec![0u8; 0x3000];

    // UNWIND_INFO @ UNWIND_RVA: version 1, no flags, prolog size 6, no frame reg.
    image[UNWIND_RVA as usize] = 0x01; // version=1, flags=0
    image[UNWIND_RVA as usize + 1] = 6; // SizeOfProlog
    image[UNWIND_RVA as usize + 2] = 3; // CountOfCodes
    image[UNWIND_RVA as usize + 3] = 0; // FrameRegister=0
                                        // codes (most-recent-prolog-op first):
    put_u16(&mut image, UNWIND_RVA + 4, mk_code(6, UWOP_ALLOC_SMALL, 3)); // sub rsp,0x20
    put_u16(
        &mut image,
        UNWIND_RVA + 6,
        mk_code(2, UWOP_PUSH_NONVOL, REG_RBX as u8),
    );
    put_u16(
        &mut image,
        UNWIND_RVA + 8,
        mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8),
    );

    // A separate EHANDLER UNWIND_INFO + scope table for the dispatch leg.
    const EH_UNWIND_RVA: u32 = 0x2080;
    image[EH_UNWIND_RVA as usize] = 0x01 | (UNW_FLAG_EHANDLER << 3);
    image[EH_UNWIND_RVA as usize + 1] = 4; // prolog size
    image[EH_UNWIND_RVA as usize + 2] = 1; // one code
    image[EH_UNWIND_RVA as usize + 3] = 0;
    put_u16(
        &mut image,
        EH_UNWIND_RVA + 4,
        mk_code(4, UWOP_ALLOC_SMALL, 0),
    );
    // padded to 2 nodes; tail at +4+4 = +8 → handler RVA then data RVA.
    put_u32(&mut image, EH_UNWIND_RVA + 8, 0x9999); // handler RVA (__C_specific_handler stand-in)
    let eh_data_rva = EH_UNWIND_RVA + 12;
    let _ = SCOPE_RVA;
    // SCOPE_TABLE inline at eh_data_rva: 1 record covering [0x1100,0x1140).
    put_u32(&mut image, eh_data_rva, 1);
    put_u32(&mut image, eh_data_rva + 4, 0x1100);
    put_u32(&mut image, eh_data_rva + 8, 0x1140);
    put_u32(&mut image, eh_data_rva + 12, 0x4000); // handler funclet
    put_u32(&mut image, eh_data_rva + 16, 0x1200); // jump target

    // Guest stack model. Entry RSP E; body RSP = E - 0x30 after the prolog.
    const E: u64 = 0x9000;
    const STACK_BASE: u64 = 0x8E00;
    let saved_rbx: u64 = 0x0000_0000_BBBB_BBBB;
    let saved_rbp: u64 = 0x0000_0000_BBBB_00BB;
    let ret_addr: u64 = 0x1_4000_5000;
    let mut stack = alloc::vec![0u8; 0x400];
    let put_stack = |stk: &mut alloc::vec::Vec<u8>, addr: u64, v: u64| {
        let off = (addr - STACK_BASE) as usize;
        stk[off..off + 8].copy_from_slice(&v.to_le_bytes());
    };
    put_stack(&mut stack, E - 0x10, saved_rbx); // popped by push-rbx unwind
    put_stack(&mut stack, E - 0x08, saved_rbp); // popped by push-rbp unwind
    put_stack(&mut stack, E, ret_addr); // return address
    let mem = SliceMemory {
        base: STACK_BASE,
        bytes: &stack,
    };

    // 1. .pdata lookup.
    let funcs = [RuntimeFunction {
        begin: FUNC_RVA,
        end: FUNC_RVA + 0x100,
        unwind_info: UNWIND_RVA,
    }];
    if lookup_function(&funcs, FUNC_RVA + 0x40).map(|f| f.unwind_info) != Some(UNWIND_RVA) {
        return false;
    }
    if lookup_function(&funcs, 0x5000).is_some() {
        return false;
    }

    // 2. Virtual unwind from a body fault.
    let mut ctx = RegContext::default();
    ctx.rip = IMAGE_BASE + (FUNC_RVA as u64) + 0x40;
    ctx.gpr[REG_RSP] = E - 0x30;
    let rf = funcs[0];
    if virtual_unwind(&image, IMAGE_BASE, &mut ctx, &rf, &mem).is_err() {
        return false;
    }
    if ctx.gpr[REG_RBX] != saved_rbx
        || ctx.gpr[REG_RBP] != saved_rbp
        || ctx.rip != ret_addr
        || ctx.gpr[REG_RSP] != E + 8
    {
        return false;
    }

    // 3. UNWIND_INFO handler parse + scope-table search.
    let eh = match parse_unwind_info(&image, EH_UNWIND_RVA) {
        Some(u) => u,
        None => return false,
    };
    if !eh.has_exception_handler() || eh.handler_rva != Some(0x9999) {
        return false;
    }
    let hit = find_scope(&image, eh.handler_data_rva, 0x1120);
    let miss = find_scope(&image, eh.handler_data_rva, 0x1000);
    match (hit, miss) {
        (Some(r), None) => r.target == 0x1200 && r.handler == 0x4000,
        _ => false,
    }
}

#[inline]
fn mk_code(offset: u8, op: u8, op_info: u8) -> u16 {
    (offset as u16) | (((op & 0x0F) as u16) << 8) | (((op_info & 0x0F) as u16) << 12)
}

#[inline]
fn put_u16(buf: &mut [u8], off: u32, v: u16) {
    let o = off as usize;
    buf[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
#[inline]
fn put_u32(buf: &mut [u8], off: u32, v: u32) {
    let o = off as usize;
    buf[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Small builder for a mapped-image byte buffer addressed by RVA.
    struct Img {
        bytes: Vec<u8>,
    }
    impl Img {
        fn new(size: usize) -> Self {
            Img {
                bytes: vec![0u8; size],
            }
        }
        fn u8(&mut self, rva: u32, v: u8) {
            self.bytes[rva as usize] = v;
        }
        fn u16(&mut self, rva: u32, v: u16) {
            put_u16(&mut self.bytes, rva, v);
        }
        fn u32(&mut self, rva: u32, v: u32) {
            put_u32(&mut self.bytes, rva, v);
        }
    }

    /// A guest stack buffer mapped at `base`.
    struct Stack {
        base: u64,
        bytes: Vec<u8>,
    }
    impl Stack {
        fn new(base: u64, size: usize) -> Self {
            Stack {
                base,
                bytes: vec![0u8; size],
            }
        }
        fn put(&mut self, addr: u64, v: u64) {
            let off = (addr - self.base) as usize;
            self.bytes[off..off + 8].copy_from_slice(&v.to_le_bytes());
        }
        fn reader(&self) -> SliceMemory<'_> {
            SliceMemory {
                base: self.base,
                bytes: &self.bytes,
            }
        }
    }

    const IB: u64 = 0x1_4000_0000;

    /// Write a v1 UNWIND_INFO header + codes at `rva`. Returns the RVA of the
    /// tail (where a handler RVA or chain record would go).
    fn write_unwind(
        img: &mut Img,
        rva: u32,
        flags: u8,
        prolog: u8,
        frame_reg: u8,
        frame_off: u8,
        codes: &[u16],
    ) -> u32 {
        img.u8(rva, 0x01 | (flags << 3));
        img.u8(rva + 1, prolog);
        img.u8(rva + 2, codes.len() as u8);
        img.u8(rva + 3, (frame_reg & 0xF) | (frame_off << 4));
        for (i, &c) in codes.iter().enumerate() {
            img.u16(rva + 4 + (i as u32) * 2, c);
        }
        let padded = ((codes.len() as u32) + 1) & !1;
        rva + 4 + padded * 2
    }

    #[test]
    fn self_test_passes() {
        assert!(run_self_test());
    }

    #[test]
    fn node_counts_match_spec() {
        assert_eq!(node_count(UWOP_PUSH_NONVOL, 5), 1);
        assert_eq!(node_count(UWOP_ALLOC_LARGE, 0), 2);
        assert_eq!(node_count(UWOP_ALLOC_LARGE, 1), 3);
        assert_eq!(node_count(UWOP_ALLOC_SMALL, 7), 1);
        assert_eq!(node_count(UWOP_SET_FPREG, 0), 1);
        assert_eq!(node_count(UWOP_SAVE_NONVOL, 3), 2);
        assert_eq!(node_count(UWOP_SAVE_NONVOL_FAR, 3), 3);
        assert_eq!(node_count(UWOP_SAVE_XMM128, 0), 2);
        assert_eq!(node_count(UWOP_SAVE_XMM128_FAR, 0), 3);
        assert_eq!(node_count(UWOP_PUSH_MACHFRAME, 1), 1);
    }

    /// `push rbp; push rbx; sub rsp,0x20` — the workhorse prolog.
    #[test]
    fn unwind_push_nonvols_and_alloc() {
        let mut img = Img::new(0x3000);
        write_unwind(
            &mut img,
            0x2000,
            0,
            6,
            0,
            0,
            &[
                mk_code(6, UWOP_ALLOC_SMALL, 3), // sub rsp,0x20
                mk_code(2, UWOP_PUSH_NONVOL, REG_RBX as u8),
                mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8),
            ],
        );
        let e = 0xA000u64;
        let mut stk = Stack::new(0x9000, 0x2000);
        stk.put(e - 0x10, 0xB17); // rbx
        stk.put(e - 0x08, 0xB09); // rbp
        stk.put(e, 0x1_4000_7777); // ret
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        ctx.rip = IB + 0x1050;
        ctx.gpr[REG_RSP] = e - 0x30;
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.gpr[REG_RBX], 0xB17);
        assert_eq!(ctx.gpr[REG_RBP], 0xB09);
        assert_eq!(ctx.rip, 0x1_4000_7777);
        assert_eq!(ctx.gpr[REG_RSP], e + 8);
    }

    /// Frame-pointer prolog: `push rbp; mov rbp,rsp; sub rsp,0x40`. After the
    /// body has pushed more, only the frame register lets us recover RSP.
    #[test]
    fn unwind_with_frame_pointer() {
        let mut img = Img::new(0x3000);
        // codes: SET_FPREG (offset 4), PUSH rbp (offset 1).
        write_unwind(
            &mut img,
            0x2000,
            0,
            8,
            REG_RBP as u8,
            0, // frame offset 0 → rbp == rsp at establishment
            &[
                mk_code(8, UWOP_ALLOC_SMALL, 7), // sub rsp,0x40
                mk_code(4, UWOP_SET_FPREG, 0),
                mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8),
            ],
        );
        let e = 0xC000u64;
        // After prolog: rbp = e-8 (points at saved rbp slot+? ) Let's define:
        // push rbp -> rsp=e-8, [e-8]=saved_rbp. mov rbp,rsp -> rbp=e-8.
        // sub rsp,0x40 -> rsp=e-0x48. Body may push more; rsp now arbitrary, but
        // SET_FPREG recovers rsp = rbp - 0 = e-8, then PUSH_NONVOL pops rbp.
        let mut stk = Stack::new(0xB000, 0x2000);
        stk.put(e - 0x08, 0xBBBB); // saved rbp
        stk.put(e, 0x1_4000_1234); // ret
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        ctx.rip = IB + 0x1080;
        ctx.gpr[REG_RBP] = e - 0x08; // established frame pointer
        ctx.gpr[REG_RSP] = e - 0x200; // body pushed a lot; value shouldn't matter
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.gpr[REG_RBP], 0xBBBB);
        assert_eq!(ctx.rip, 0x1_4000_1234);
        assert_eq!(ctx.gpr[REG_RSP], e + 8);
    }

    /// `mov [rsp+off],r12` style register saves (UWOP_SAVE_NONVOL) read from the
    /// frame base, not by popping.
    #[test]
    fn unwind_save_nonvol_offset() {
        let mut img = Img::new(0x3000);
        // sub rsp,0x40 (alloc small op_info 7); save r12 at [rsp+0x20] (off/8=4).
        write_unwind(
            &mut img,
            0x2000,
            0,
            12,
            0,
            0,
            &[
                mk_code(12, UWOP_SAVE_NONVOL, REG_R12 as u8),
                4,                               // operand: offset/8 = 4 → 0x20
                mk_code(4, UWOP_ALLOC_SMALL, 7), // sub rsp,0x40
            ],
        );
        let e = 0xD000u64;
        // No frame reg → frame_base = entry rsp = body rsp. Body rsp = e-0x48
        // (ret pushed by call at [e], then sub 0x40 ... but no pushes here; the
        // call put ret at [e], so entry rsp = e, alloc 0x40 → body rsp = e-0x40).
        let body_rsp = e - 0x40;
        let mut stk = Stack::new(0xC000, 0x2000);
        stk.put(body_rsp + 0x20, 0x12_12_12_12); // saved r12 at frame_base+0x20
        stk.put(e, 0x1_4000_9090); // ret at [e]
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        ctx.rip = IB + 0x1060;
        ctx.gpr[REG_RSP] = body_rsp;
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.gpr[REG_R12], 0x12_12_12_12);
        assert_eq!(ctx.rip, 0x1_4000_9090);
        assert_eq!(ctx.gpr[REG_RSP], e + 8);
    }

    /// ALLOC_LARGE with op_info 0 (size/8 in one node) and op_info 1 (full u32).
    #[test]
    fn unwind_alloc_large_both_forms() {
        // op_info 0: alloc 0x800 → size/8 = 0x100.
        {
            let mut img = Img::new(0x3000);
            write_unwind(
                &mut img,
                0x2000,
                0,
                7,
                0,
                0,
                &[mk_code(7, UWOP_ALLOC_LARGE, 0), 0x100],
            );
            let e = 0xE000u64;
            let mut stk = Stack::new(0xD000, 0x2000);
            stk.put(e, 0x1_4000_AAAA);
            let rf = RuntimeFunction {
                begin: 0x1000,
                end: 0x1100,
                unwind_info: 0x2000,
            };
            let mut ctx = RegContext::default();
            ctx.rip = IB + 0x1050;
            ctx.gpr[REG_RSP] = e - 0x800;
            virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
            assert_eq!(ctx.rip, 0x1_4000_AAAA);
            assert_eq!(ctx.gpr[REG_RSP], e + 8);
        }
        // op_info 1: alloc 0x12340 → low 0x2340, high 0x1.
        {
            let mut img = Img::new(0x3000);
            write_unwind(
                &mut img,
                0x2000,
                0,
                7,
                0,
                0,
                &[mk_code(7, UWOP_ALLOC_LARGE, 1), 0x2340, 0x1],
            );
            let e = 0x10_0000u64;
            let mut stk = Stack::new(0x8000, 0x100000);
            stk.put(e, 0x1_4000_CCCC);
            let rf = RuntimeFunction {
                begin: 0x1000,
                end: 0x1100,
                unwind_info: 0x2000,
            };
            let mut ctx = RegContext::default();
            ctx.rip = IB + 0x1050;
            ctx.gpr[REG_RSP] = e - 0x12340;
            virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
            assert_eq!(ctx.rip, 0x1_4000_CCCC);
            assert_eq!(ctx.gpr[REG_RSP], e + 8);
        }
    }

    /// PUSH_MACHFRAME (interrupt entry): RIP and RSP come from the trap frame,
    /// no extra return-address pop.
    #[test]
    fn unwind_push_machframe() {
        let mut img = Img::new(0x3000);
        write_unwind(
            &mut img,
            0x2000,
            0,
            0,
            0,
            0,
            &[mk_code(0, UWOP_PUSH_MACHFRAME, 0)],
        );
        let base = 0xF000u64;
        let mut stk = Stack::new(0xE000, 0x2000);
        stk.put(base, 0x1_4000_5555); // RIP at [rsp+0]
        stk.put(base + 0x18, 0x2_0000); // OldRsp at [rsp+0x18]
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        ctx.rip = IB + 0x1000;
        ctx.gpr[REG_RSP] = base;
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.rip, 0x1_4000_5555);
        assert_eq!(ctx.gpr[REG_RSP], 0x2_0000);
    }

    /// A fault *inside* the prolog must skip codes for instructions that haven't
    /// executed yet. Here only `push rbp` has run when the fault hits at off 1.
    #[test]
    fn partial_prolog_skips_unexecuted_codes() {
        let mut img = Img::new(0x3000);
        write_unwind(
            &mut img,
            0x2000,
            0,
            6,
            0,
            0,
            &[
                mk_code(6, UWOP_ALLOC_SMALL, 3),
                mk_code(2, UWOP_PUSH_NONVOL, REG_RBX as u8),
                mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8),
            ],
        );
        let e = 0x11000u64;
        let mut stk = Stack::new(0x10000, 0x2000);
        stk.put(e - 0x08, 0xB09); // saved rbp (only push that executed)
        stk.put(e, 0x1_4000_2222); // ret
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        // Fault at function offset 1: only `push rbp` (code_offset 1) executed.
        ctx.rip = IB + 0x1001;
        ctx.gpr[REG_RSP] = e - 0x08;
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.gpr[REG_RBP], 0xB09);
        assert_eq!(ctx.rip, 0x1_4000_2222);
        assert_eq!(ctx.gpr[REG_RSP], e + 8);
    }

    /// Chained unwind info: a parent UNWIND_INFO whose codes also fold.
    #[test]
    fn unwind_follows_chaininfo() {
        let mut img = Img::new(0x4000);
        // Primary @0x2000: push rbx (offset 1), then CHAININFO to parent @0x3000.
        let tail = write_unwind(
            &mut img,
            0x2000,
            UNW_FLAG_CHAININFO,
            1,
            0,
            0,
            &[mk_code(1, UWOP_PUSH_NONVOL, REG_RBX as u8)],
        );
        // chained RUNTIME_FUNCTION at tail → parent func.
        img.u32(tail, 0x1000);
        img.u32(tail + 4, 0x1100);
        img.u32(tail + 8, 0x3000);
        // Parent @0x3000: push rbp (offset 1), no flags.
        write_unwind(
            &mut img,
            0x3000,
            0,
            1,
            0,
            0,
            &[mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8)],
        );
        let e = 0x13000u64;
        let mut stk = Stack::new(0x12000, 0x2000);
        stk.put(e - 0x10, 0xB17); // rbx (primary pops first)
        stk.put(e - 0x08, 0xB09); // rbp (parent pops)
        stk.put(e, 0x1_4000_3333); // ret
        let rf = RuntimeFunction {
            begin: 0x1000,
            end: 0x1100,
            unwind_info: 0x2000,
        };
        let mut ctx = RegContext::default();
        ctx.rip = IB + 0x1050;
        ctx.gpr[REG_RSP] = e - 0x10;
        virtual_unwind(&img.bytes, IB, &mut ctx, &rf, &stk.reader()).unwrap();
        assert_eq!(ctx.gpr[REG_RBX], 0xB17);
        assert_eq!(ctx.gpr[REG_RBP], 0xB09);
        assert_eq!(ctx.rip, 0x1_4000_3333);
        assert_eq!(ctx.gpr[REG_RSP], e + 8);
    }

    #[test]
    fn pdata_parse_and_lookup() {
        let mut img = Img::new(0x3000);
        // Two RUNTIME_FUNCTIONs in .pdata @ 0x100.
        let pd = 0x100u32;
        img.u32(pd, 0x1000);
        img.u32(pd + 4, 0x1100);
        img.u32(pd + 8, 0x2000);
        img.u32(pd + 12, 0x1100);
        img.u32(pd + 16, 0x1200);
        img.u32(pd + 20, 0x2010);
        let funcs = parse_pdata(&img.bytes, pd, 24);
        assert_eq!(funcs.len(), 2);
        assert_eq!(lookup_function(&funcs, 0x1000).unwrap().unwind_info, 0x2000);
        assert_eq!(lookup_function(&funcs, 0x10FF).unwrap().unwind_info, 0x2000);
        assert_eq!(lookup_function(&funcs, 0x1100).unwrap().unwind_info, 0x2010);
        assert_eq!(lookup_function(&funcs, 0x11FF).unwrap().unwind_info, 0x2010);
        assert!(lookup_function(&funcs, 0x0FFF).is_none());
        assert!(lookup_function(&funcs, 0x1200).is_none());
    }

    /// Full search-phase dispatch: inner frame has no handler, the caller frame
    /// has an EHANDLER whose scope table covers the (unwound) return address.
    #[test]
    fn dispatch_finds_handler_in_caller() {
        let mut img = Img::new(0x4000);

        // .pdata @ 0x100: inner func [0x1000,0x1100) uw@0x2000; outer func
        // [0x1100,0x1200) uw@0x3000 (the caller, with the handler).
        let pd = 0x100u32;
        img.u32(pd, 0x1000);
        img.u32(pd + 4, 0x1100);
        img.u32(pd + 8, 0x2000);
        img.u32(pd + 12, 0x1100);
        img.u32(pd + 16, 0x1200);
        img.u32(pd + 20, 0x3000);
        let funcs = parse_pdata(&img.bytes, pd, 24);

        // Inner @0x2000: push rbp, no handler.
        write_unwind(
            &mut img,
            0x2000,
            0,
            1,
            0,
            0,
            &[mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8)],
        );
        // Outer @0x3000: EHANDLER + scope table covering [0x1150,0x1180).
        let tail = write_unwind(
            &mut img,
            0x3000,
            UNW_FLAG_EHANDLER,
            0,
            0,
            0,
            &[mk_code(0, UWOP_ALLOC_SMALL, 0)],
        );
        img.u32(tail, 0x7777); // handler RVA
        let data = tail + 4;
        img.u32(data, 1); // scope count
        img.u32(data + 4, 0x1150);
        img.u32(data + 8, 0x1180);
        img.u32(data + 12, 0x8000); // handler funclet
        img.u32(data + 16, 0x1190); // jump target

        // Stack: inner frame pushed rbp then the call into inner put the return
        // address (inside outer, at rva 0x1160) on the stack.
        let e = 0x15000u64;
        let mut stk = Stack::new(0x14000, 0x2000);
        let ret_into_outer = IB + 0x1160;
        stk.put(e - 0x08, 0xB09); // saved rbp
        stk.put(e, ret_into_outer); // return address → inside outer's scope
        let mem = stk.reader();

        let mut fault = RegContext::default();
        fault.rip = IB + 0x1050; // faulting in inner
        fault.gpr[REG_RSP] = e - 0x08; // after inner's push rbp

        match dispatch_exception(&img.bytes, IB, &funcs, &mem, &fault) {
            Disposition::HandlerFound {
                handler_rva,
                scope,
                frame_rip,
                ..
            } => {
                assert_eq!(handler_rva, 0x7777);
                assert_eq!(frame_rip, ret_into_outer);
                let s = scope.expect("scope record");
                assert_eq!(s.target, 0x1190);
                assert_eq!(s.handler, 0x8000);
            }
            other => panic!("expected HandlerFound, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_unhandled_when_no_handler_anywhere() {
        let mut img = Img::new(0x3000);
        let pd = 0x100u32;
        img.u32(pd, 0x1000);
        img.u32(pd + 4, 0x1100);
        img.u32(pd + 8, 0x2000);
        let funcs = parse_pdata(&img.bytes, pd, 12);
        write_unwind(
            &mut img,
            0x2000,
            0,
            1,
            0,
            0,
            &[mk_code(1, UWOP_PUSH_NONVOL, REG_RBP as u8)],
        );
        // Return address unwinds to RVA 0x5000 which has no .pdata entry.
        let e = 0x17000u64;
        let mut stk = Stack::new(0x16000, 0x2000);
        stk.put(e - 0x08, 0);
        stk.put(e, IB + 0x5000);
        let mem = stk.reader();
        let mut fault = RegContext::default();
        fault.rip = IB + 0x1050;
        fault.gpr[REG_RSP] = e - 0x08;
        match dispatch_exception(&img.bytes, IB, &funcs, &mem, &fault) {
            Disposition::NoUnwindInfo { rip } => assert_eq!(rip, IB + 0x5000),
            other => panic!("expected NoUnwindInfo, got {:?}", other),
        }
    }

    #[test]
    fn truncated_unwind_info_is_rejected() {
        let img = Img::new(0x100);
        // Nothing written → version byte 0 → unsupported version → None.
        assert!(parse_unwind_info(&img.bytes, 0x10).is_none());
        // Out-of-range RVA → None, no panic.
        assert!(parse_unwind_info(&img.bytes, 0xFFFF).is_none());
    }

    #[test]
    fn corrupt_scope_count_is_bounded() {
        // Image holds the count but not even one full 16-byte record after it: a
        // hostile count must stop at the truncation, never spin or read OOB.
        let mut img = Img::new(0x14);
        img.u32(0x10, 0xFFFF_FFFF); // absurd count, no record bytes follow
        let recs = parse_scope_table(&img.bytes, 0x10);
        assert!(recs.is_empty());
        // And with records present, the 4096 cap bounds a corrupt count.
        let mut big = Img::new(0x2000);
        big.u32(0x10, 0xFFFF_FFFF);
        let bounded = parse_scope_table(&big.bytes, 0x10);
        assert!(bounded.len() <= 4096);
    }
}
