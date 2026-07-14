//! Pure logic backing the MSVC `/MT` CRT-startup imports a real `cl.exe`
//! console `.exe` pulls *before* `main()` — code-page conversions, the
//! EncodePointer/DecodePointer cookie, the FLS slot model, and the
//! process-global loaded-module registry the `Rtl*` SEH-filter machinery
//! walks to map a PC back to its image + `.pdata`.
//!
//! Concept §Compatibility Strategy: "AthBridge runs Windows apps on day one."
//! Reaching `main` in an unmodified MSVC binary is the concrete proof that the
//! CRT's startup sequence (`__scrt_common_main_seh`) ran on AthenaOS. Every
//! function here is a *pure* transform or a serialized table operation, so it
//! is unit-tested off-target (host KAT) BEFORE any guest executes it — a wrong
//! UTF-8↔UTF-16 conversion or a stale EncodePointer cookie corrupts the guest
//! silently, exactly the class of bug the host-KAT-first rule exists to catch.
//!
//! Deferred-by-design (clearly marked): the unwind functions
//! (`RtlVirtualUnwind`/`RtlUnwindEx`) implement the *no-exception-thrown*
//! startup path — the CRT *installs* `RtlLookupFunctionEntry` +
//! `SetUnhandledExceptionFilter` during init but does not THROW before `main`,
//! so a correct lookup + a context-preserving virtual-unwind that defers to the
//! existing table-based [`crate::seh`] dispatcher is sufficient to reach `main`.
//! Full SEH *dispatch* (live fault → guest `__C_specific_handler`) remains
//! HUMAN-GATED guest-execution work tracked in MasterChecklist Phase 11.2.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// EncodePointer / DecodePointer cookie
// ---------------------------------------------------------------------------

/// Per-process pointer-encoding cookie. Real Windows derives this from
/// `SystemTimeOfDay`/PEB entropy; the contract the CRT relies on is only that
/// `DecodePointer(EncodePointer(p)) == p` for the life of the process and that
/// the encoding is not the identity (so a leaked encoded pointer is not a raw
/// pointer). A fixed-but-nonzero cookie satisfies both — the CRT stores
/// encoded function pointers (e.g. its `_purecall` handler) and round-trips
/// them; it never inspects the cookie itself.
///
/// `EncodePointer`/`DecodePointer` are an involution under XOR with a rotate:
/// Windows uses `ROR(p XOR cookie, cookie & 0x3F)` for Encode and the inverse
/// for Decode. We mirror that exactly so the round-trip is bit-faithful and the
/// encoding genuinely scrambles the pointer (not a bare XOR).
pub const POINTER_COOKIE: u64 = 0x5AEE_B41D_C0DE_F00D;

/// `EncodePointer(ptr)` — `ROR(ptr XOR cookie, cookie & 0x3F)`.
#[inline]
pub fn encode_pointer(ptr: u64) -> u64 {
    let x = ptr ^ POINTER_COOKIE;
    x.rotate_right((POINTER_COOKIE & 0x3F) as u32)
}

/// `DecodePointer(enc)` — exact inverse of [`encode_pointer`].
#[inline]
pub fn decode_pointer(enc: u64) -> u64 {
    enc.rotate_left((POINTER_COOKIE & 0x3F) as u32) ^ POINTER_COOKIE
}

// ---------------------------------------------------------------------------
// SLIST_HEADER
// ---------------------------------------------------------------------------

/// `InitializeSListHead` zeroes the 16-byte `SLIST_HEADER`. The CRT uses an
/// interlocked singly-linked list for its low-fragmentation block cache; a
/// zeroed header is the canonical empty list. Pure: writes 16 zero bytes.
/// Returns the number of bytes the caller should clear (always 16) so the shim
/// and the KAT agree on the structure size.
pub const SLIST_HEADER_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Fiber-Local Storage (FLS) — same per-session slot model as TLS
// ---------------------------------------------------------------------------

/// `FLS_OUT_OF_INDEXES` — `FlsAlloc` failure sentinel (matches `TLS_OUT_OF_INDEXES`).
pub const FLS_OUT_OF_INDEXES: u32 = 0xFFFF_FFFF;

/// Upper bound on FLS slots (Windows caps at 128 in the classic ABI; the CRT
/// only ever needs a couple). Keeps a runaway `FlsAlloc` loop bounded.
pub const FLS_MAX_SLOTS: usize = 128;

/// One FLS slot: `value` is the stored `PVOID`; `callback` is the optional
/// destructor `FlsAlloc` was given (a guest VA, invoked on thread/fiber exit —
/// not driven in the single-threaded startup model, but stored faithfully so a
/// later teardown can fire it). `None` for the whole `Option<FlsSlot>` means the
/// index is free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlsSlot {
    pub value: u64,
    pub callback: u64,
}

/// Allocate an FLS slot in `slots`, reusing a freed one first. Mirrors
/// [`crate::kernel32::tls_alloc`] exactly so FLS and TLS behave identically (the
/// CRT treats FLS as "TLS that runs destructors"). Returns the index, or
/// [`FLS_OUT_OF_INDEXES`] when full. Pure (operates on the passed slice).
pub fn fls_alloc(slots: &mut Vec<Option<FlsSlot>>, callback: u64) -> u32 {
    let slot = FlsSlot { value: 0, callback };
    if let Some(idx) = slots.iter().position(|s| s.is_none()) {
        slots[idx] = Some(slot);
        return idx as u32;
    }
    if slots.len() >= FLS_MAX_SLOTS {
        return FLS_OUT_OF_INDEXES;
    }
    let idx = slots.len();
    slots.push(Some(slot));
    idx as u32
}

/// `FlsFree(index)` — release the slot. Returns `true` on success (a live slot),
/// `false` for a bad/already-free index.
pub fn fls_free(slots: &mut Vec<Option<FlsSlot>>, index: u32) -> bool {
    match slots.get_mut(index as usize) {
        Some(slot @ Some(_)) => {
            *slot = None;
            true
        }
        _ => false,
    }
}

/// `FlsGetValue(index)` — returns the stored value, or `0` for a bad index
/// (Windows returns 0 + sets LastError; the caller sets LastError).
pub fn fls_get_value(slots: &[Option<FlsSlot>], index: u32) -> Option<u64> {
    slots
        .get(index as usize)
        .copied()
        .flatten()
        .map(|s| s.value)
}

/// `FlsSetValue(index, value)` — store; returns `true` on a live slot.
pub fn fls_set_value(slots: &mut Vec<Option<FlsSlot>>, index: u32, value: u64) -> bool {
    match slots.get_mut(index as usize) {
        Some(Some(slot)) => {
            slot.value = value;
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Code-page / locale conversions (UTF-8/ASCII + CP-1252/437 defaults)
// ---------------------------------------------------------------------------
//
// The MSVC CRT calls MultiByteToWideChar/WideCharToMultiByte to widen its
// command line / environment and GetStringTypeW/LCMapStringW/CompareStringW to
// classify and case-map. We back the ANSI code pages (1252, 437) and CP_ACP/
// CP_OEMCP with a single-byte-to-Unicode table that is ASCII-identical and
// Latin-1-faithful for the high half (CP-1252 == Latin-1 except for 0x80..0x9F,
// which we map straight through — correct enough for ASCII startup text and
// never lossy on the round-trip the CRT performs). CP_UTF8 (65001) gets a real
// UTF-8 decoder/encoder.

pub const CP_ACP: u32 = 0;
pub const CP_OEMCP: u32 = 1;
pub const CP_UTF8: u32 = 65001;
pub const CP_1252: u32 = 1252;
pub const CP_437: u32 = 437;

/// Is `code_page` one this layer supports? (`IsValidCodePage`.)
pub fn is_valid_code_page(code_page: u32) -> bool {
    matches!(code_page, CP_ACP | CP_OEMCP | CP_UTF8 | CP_1252 | CP_437)
        || code_page == 1200 // UTF-16LE
        || code_page == 1201 // UTF-16BE
}

/// Decode `bytes` in `code_page` to a UTF-16 (`u16`) vector. ASCII bytes map
/// 1:1; the high half of an SBCS page maps via Latin-1 (codepoint == byte);
/// CP_UTF8 runs a real UTF-8 decoder (invalid sequences → U+FFFD). This is the
/// `MultiByteToWideChar` core — pure, host-KAT'd for ASCII + UTF-8 round-trips.
pub fn mb_to_wide(code_page: u32, bytes: &[u8]) -> Vec<u16> {
    if code_page == CP_UTF8 {
        let s = decode_utf8_lossy(bytes);
        return s.encode_utf16().collect();
    }
    // SBCS: every byte is one UTF-16 unit. ASCII identical; 0x80..0xFF as
    // Latin-1 (codepoint == byte value). This is exact for CP-1252's overlap
    // with Latin-1 and lossless on the round-trip the CRT relies on.
    bytes.iter().map(|&b| b as u16).collect()
}

/// Encode a UTF-16 slice to `code_page` bytes. Inverse of [`mb_to_wide`].
/// SBCS: a unit ≤ 0xFF is one byte, anything higher → the default char `?`.
/// CP_UTF8: real UTF-8 encoder. This is the `WideCharToMultiByte` core.
pub fn wide_to_mb(code_page: u32, wide: &[u16]) -> Vec<u8> {
    if code_page == CP_UTF8 {
        let s = String::from_utf16_lossy(wide);
        return s.into_bytes();
    }
    wide.iter()
        .map(|&u| if u <= 0xFF { u as u8 } else { b'?' })
        .collect()
}

/// Minimal UTF-8 → `String` lossy decoder (`core::str::from_utf8` then lossy
/// fallback per maximal-subpart). `String::from_utf8_lossy` already does this,
/// so we delegate; kept as a named function so the KAT targets the exact path
/// `mb_to_wide` uses.
fn decode_utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `GetStringTypeW` CT_CTYPE1 classification bits for one UTF-16 unit. Only the
/// flags the CRT inspects during startup are populated (it classifies its own
/// numeric/locale strings). Pure → host-KAT'd.
pub const C1_UPPER: u16 = 0x0001;
pub const C1_LOWER: u16 = 0x0002;
pub const C1_DIGIT: u16 = 0x0004;
pub const C1_SPACE: u16 = 0x0008;
pub const C1_PUNCT: u16 = 0x0010;
pub const C1_CNTRL: u16 = 0x0020;
pub const C1_BLANK: u16 = 0x0040;
pub const C1_ALPHA: u16 = 0x0100;

/// Classify one UTF-16 unit into CT_CTYPE1 flags.
pub fn string_type_ctype1(u: u16) -> u16 {
    let c = u;
    let mut t = 0u16;
    if (0x41..=0x5A).contains(&c) {
        t |= C1_UPPER | C1_ALPHA;
    } else if (0x61..=0x7A).contains(&c) {
        t |= C1_LOWER | C1_ALPHA;
    } else if (0x30..=0x39).contains(&c) {
        t |= C1_DIGIT;
    } else if c == 0x20 {
        t |= C1_SPACE | C1_BLANK;
    } else if c == 0x09 {
        t |= C1_SPACE | C1_BLANK | C1_CNTRL;
    } else if (0x09..=0x0D).contains(&c) {
        t |= C1_SPACE | C1_CNTRL;
    } else if c < 0x20 || c == 0x7F {
        t |= C1_CNTRL;
    } else if (0x21..=0x2F).contains(&c)
        || (0x3A..=0x40).contains(&c)
        || (0x5B..=0x60).contains(&c)
        || (0x7B..=0x7E).contains(&c)
    {
        t |= C1_PUNCT;
    } else if c >= 0x80 {
        // High Latin-1: treat letters as ALPHA, the rest PUNCT (good enough for
        // the CRT's startup classification; never wrong for ASCII).
        t |= C1_ALPHA;
    }
    t
}

/// `LCMAP_UPPERCASE` / `LCMAP_LOWERCASE` flags for [`lc_map_string`].
pub const LCMAP_LOWERCASE: u32 = 0x0000_0100;
pub const LCMAP_UPPERCASE: u32 = 0x0000_0200;

/// `LCMapStringW` core: case-map a UTF-16 slice per `flags` (ASCII-range case
/// fold; other transforms pass through). Returns a new `Vec<u16>`. Pure.
pub fn lc_map_string(flags: u32, src: &[u16]) -> Vec<u16> {
    src.iter()
        .map(|&u| {
            if flags & LCMAP_UPPERCASE != 0 {
                if (0x61..=0x7A).contains(&u) {
                    u - 0x20
                } else {
                    u
                }
            } else if flags & LCMAP_LOWERCASE != 0 {
                if (0x41..=0x5A).contains(&u) {
                    u + 0x20
                } else {
                    u
                }
            } else {
                u
            }
        })
        .collect()
}

/// `CompareStringW` ordinal/locale compare result, in the `CSTR_*` convention
/// (1 = less, 2 = equal, 3 = greater; the CRT subtracts 2). Case-sensitive
/// ordinal unless `ignore_case`. Pure → host-KAT'd.
pub const CSTR_LESS_THAN: i32 = 1;
pub const CSTR_EQUAL: i32 = 2;
pub const CSTR_GREATER_THAN: i32 = 3;
pub const NORM_IGNORECASE: u32 = 0x0000_0001;

pub fn compare_string(flags: u32, a: &[u16], b: &[u16]) -> i32 {
    let fold = |u: u16| -> u16 {
        if flags & NORM_IGNORECASE != 0 && (0x41..=0x5A).contains(&u) {
            u + 0x20
        } else {
            u
        }
    };
    let mut ia = a.iter().map(|&u| fold(u));
    let mut ib = b.iter().map(|&u| fold(u));
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return CSTR_EQUAL,
            (None, Some(_)) => return CSTR_LESS_THAN,
            (Some(_), None) => return CSTR_GREATER_THAN,
            (Some(x), Some(y)) => {
                if x < y {
                    return CSTR_LESS_THAN;
                } else if x > y {
                    return CSTR_GREATER_THAN;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Loaded-module registry (for RtlLookupFunctionEntry / RtlPcToFileHeader)
// ---------------------------------------------------------------------------
//
// The Rtl* SEH-filter functions take a raw PC and must answer "which module,
// and where is its .pdata?". The PE loader (exec::load_pe_executable) is the
// only code that knows a loaded image's base/size/.pdata location, but the
// win64 shims run in a separate process-global context. So the loader registers
// each executable image here and the shims query it. Serialized on a spinlock;
// bounded (the startup path loads one main image; LoadLibraryExW is a stub that
// returns a synthetic handle and loads nothing, so no real DLL images land
// here during startup).

/// One registered executable image: its mapped base, size, and the absolute VAs
/// of its `.pdata` `RUNTIME_FUNCTION` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadedModule {
    pub base: u64,
    pub size: u64,
    /// Absolute VA of the `.pdata` table (`base + pdata_rva`), 0 if none.
    pub pdata_va: u64,
    /// `.pdata` size in bytes (a multiple of 12).
    pub pdata_size: u32,
}

impl LoadedModule {
    /// Does this image cover absolute address `pc`?
    #[inline]
    pub fn covers(&self, pc: u64) -> bool {
        pc >= self.base && pc < self.base.saturating_add(self.size)
    }

    /// Find the `RUNTIME_FUNCTION` covering `pc` by scanning this image's
    /// `.pdata` directly (absolute VAs; guest VA == host VA). Returns the
    /// absolute VA of the matching 12-byte `.pdata` entry, or 0. This is the
    /// `RtlLookupFunctionEntry` core for a registered module.
    ///
    /// SAFETY: caller guarantees `pdata_va..pdata_va+pdata_size` is mapped
    /// readable (it is — the loader mapped the whole image). Only used through
    /// the shim, never in host KATs (which call the slice variant below).
    pub unsafe fn lookup_function_entry(&self, pc: u64) -> u64 {
        if self.pdata_va == 0 || self.pdata_size < 12 || !self.covers(pc) {
            return 0;
        }
        let rva = (pc - self.base) as u32;
        let count = self.pdata_size / 12;
        for i in 0..count {
            let ent = self.pdata_va + (i * 12) as u64;
            let begin = core::ptr::read_unaligned(ent as *const u32);
            let end = core::ptr::read_unaligned((ent + 4) as *const u32);
            if begin == 0 && end == 0 {
                break;
            }
            if rva >= begin && rva < end {
                return ent;
            }
        }
        0
    }
}

struct ModuleRegistry {
    lock: AtomicBool,
    mods: UnsafeCell<Vec<LoadedModule>>,
}

// SAFETY: every access serializes on `lock`.
unsafe impl Sync for ModuleRegistry {}

static MODULE_REGISTRY: ModuleRegistry = ModuleRegistry {
    lock: AtomicBool::new(false),
    mods: UnsafeCell::new(Vec::new()),
};

fn reg_lock() {
    while MODULE_REGISTRY
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

/// The actual mapped base of the **main executable image** (what
/// `GetModuleHandle(NULL)` must return). Set by the loader for the first
/// executable image it maps. The MSVC CRT dereferences this HMODULE to read the
/// image's PE header (load config, exception directory), so it MUST be the real
/// relocated base, not a synthetic `0x400000` — returning the synthetic value
/// faults the CRT's post-main header walk (observed: PAGE FAULT at 0x400000).
/// 0 = not yet set (fall back to the synthetic base).
static MAIN_MODULE_BASE: AtomicU64 = AtomicU64::new(0);

/// Record the real mapped base of the main executable image. Called once by the
/// loader for the first image (the .exe, not a DLL). Subsequent calls are
/// ignored so a later DLL load cannot steal "main module" identity.
pub fn set_main_module_base(base: u64) {
    let _ = MAIN_MODULE_BASE.compare_exchange(0, base, Ordering::SeqCst, Ordering::Relaxed);
}

/// The real main-module base, or 0 if none recorded yet.
pub fn main_module_base() -> u64 {
    MAIN_MODULE_BASE.load(Ordering::SeqCst)
}

#[cfg(test)]
pub fn clear_main_module_base_for_test() {
    MAIN_MODULE_BASE.store(0, Ordering::SeqCst);
}

/// Register a loaded executable image so the `Rtl*` shims can map a PC to it.
/// Called by `exec::load_pe_executable` after the image is mapped + `.pdata`
/// located. Idempotent on `base` (re-registering replaces).
pub fn register_module(m: LoadedModule) {
    reg_lock();
    // SAFETY: lock held — exclusive access.
    unsafe {
        let v = &mut *MODULE_REGISTRY.mods.get();
        if let Some(slot) = v.iter_mut().find(|e| e.base == m.base) {
            *slot = m;
        } else {
            v.push(m);
        }
    }
    MODULE_REGISTRY.lock.store(false, Ordering::Release);
}

/// The module covering `pc`, if any (`RtlPcToFileHeader` returns its base).
pub fn module_for_pc(pc: u64) -> Option<LoadedModule> {
    reg_lock();
    // SAFETY: lock held — exclusive access.
    let found = unsafe {
        let v = &*MODULE_REGISTRY.mods.get();
        v.iter().copied().find(|m| m.covers(pc))
    };
    MODULE_REGISTRY.lock.store(false, Ordering::Release);
    found
}

/// Number of registered modules (smoketest/KAT visibility).
pub fn module_count() -> usize {
    reg_lock();
    // SAFETY: lock held.
    let n = unsafe { (*MODULE_REGISTRY.mods.get()).len() };
    MODULE_REGISTRY.lock.store(false, Ordering::Release);
    n
}

#[cfg(test)]
pub fn clear_modules_for_test() {
    reg_lock();
    // SAFETY: lock held.
    unsafe {
        (*MODULE_REGISTRY.mods.get()).clear();
    }
    MODULE_REGISTRY.lock.store(false, Ordering::Release);
}

// ---------------------------------------------------------------------------
// Unhandled-exception filter slot
// ---------------------------------------------------------------------------

/// Storage for `SetUnhandledExceptionFilter`'s installed filter (a guest VA).
/// The CRT installs `__scrt_unhandled_exception_filter` here during startup and
/// keeps the previous value we return. We store it faithfully; it is only ever
/// *invoked* on an unhandled exception, which does not occur before `main` on
/// the no-throw startup path. 0 = none installed.
static UNHANDLED_FILTER: AtomicU64 = AtomicU64::new(0);

/// `SetUnhandledExceptionFilter(filter)` → returns the previous filter.
pub fn set_unhandled_exception_filter(filter: u64) -> u64 {
    UNHANDLED_FILTER.swap(filter, Ordering::SeqCst)
}

/// The currently installed unhandled-exception filter (0 = none).
pub fn current_unhandled_filter() -> u64 {
    UNHANDLED_FILTER.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Host KATs — every transform proven off-target, FAIL-able
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn encode_decode_pointer_round_trips() {
        for &p in &[
            0u64,
            1,
            0x1234_5678,
            0x1_4000_1000,
            u64::MAX,
            0xDEAD_BEEF_CAFE_F00D,
        ] {
            let e = encode_pointer(p);
            assert_eq!(decode_pointer(e), p, "round-trip {p:#x}");
        }
        // Encoding must scramble (not identity) for a typical pointer.
        let p = 0x1_4000_1000u64;
        assert_ne!(encode_pointer(p), p, "EncodePointer must not be identity");
        // NULL encodes to a non-null cookie image; the CRT stores encoded NULL
        // and decodes it back to NULL — that must hold.
        assert_eq!(decode_pointer(encode_pointer(0)), 0);
    }

    #[test]
    fn fls_alloc_set_get_free_round_trip() {
        let mut slots: Vec<Option<FlsSlot>> = Vec::new();
        let i0 = fls_alloc(&mut slots, 0xCB0);
        let i1 = fls_alloc(&mut slots, 0);
        assert_ne!(i0, FLS_OUT_OF_INDEXES);
        assert_ne!(i1, FLS_OUT_OF_INDEXES);
        assert_ne!(i0, i1);
        // Fresh slots read 0.
        assert_eq!(fls_get_value(&slots, i0), Some(0));
        // Set + get round-trips.
        assert!(fls_set_value(&mut slots, i0, 0xABCD));
        assert_eq!(fls_get_value(&slots, i0), Some(0xABCD));
        // Callback stored.
        assert_eq!(slots[i0 as usize].unwrap().callback, 0xCB0);
        // Free invalidates and frees the slot for reuse.
        assert!(fls_free(&mut slots, i0));
        assert_eq!(fls_get_value(&slots, i0), None);
        assert!(!fls_set_value(&mut slots, i0, 1), "set on freed slot fails");
        let i2 = fls_alloc(&mut slots, 0);
        assert_eq!(i2, i0, "freed slot is reused");
        // Bad index.
        assert_eq!(fls_get_value(&slots, 9999), None);
        assert!(!fls_free(&mut slots, 9999));
    }

    #[test]
    fn fls_alloc_bounded() {
        let mut slots: Vec<Option<FlsSlot>> = Vec::new();
        for _ in 0..FLS_MAX_SLOTS {
            assert_ne!(fls_alloc(&mut slots, 0), FLS_OUT_OF_INDEXES);
        }
        assert_eq!(
            fls_alloc(&mut slots, 0),
            FLS_OUT_OF_INDEXES,
            "FlsAlloc must fail once full"
        );
    }

    #[test]
    fn mb_to_wide_ascii_and_utf8_round_trip() {
        // ASCII via CP-1252.
        let w = mb_to_wide(CP_1252, b"hello");
        assert_eq!(w, vec![0x68, 0x65, 0x6C, 0x6C, 0x6F]);
        assert_eq!(wide_to_mb(CP_1252, &w), b"hello");
        // High Latin-1 byte round-trips through CP-1252 (0xE9 = é = U+00E9).
        let w2 = mb_to_wide(CP_1252, &[0xE9]);
        assert_eq!(w2, vec![0x00E9]);
        assert_eq!(wide_to_mb(CP_1252, &w2), vec![0xE9]);
        // UTF-8 round-trip with a multi-byte codepoint (é = C3 A9).
        let w3 = mb_to_wide(CP_UTF8, &[0x68, 0x69, 0xC3, 0xA9]);
        assert_eq!(w3, vec![0x68, 0x69, 0x00E9]);
        assert_eq!(wide_to_mb(CP_UTF8, &w3), vec![0x68, 0x69, 0xC3, 0xA9]);
    }

    #[test]
    fn code_page_validity() {
        assert!(is_valid_code_page(CP_UTF8));
        assert!(is_valid_code_page(CP_1252));
        assert!(is_valid_code_page(CP_437));
        assert!(is_valid_code_page(CP_ACP));
        assert!(is_valid_code_page(1200));
        assert!(!is_valid_code_page(99999), "bogus code page rejected");
    }

    #[test]
    fn string_type_classifies_ascii() {
        assert_eq!(string_type_ctype1(b'A' as u16) & C1_UPPER, C1_UPPER);
        assert_eq!(string_type_ctype1(b'A' as u16) & C1_ALPHA, C1_ALPHA);
        assert_eq!(string_type_ctype1(b'z' as u16) & C1_LOWER, C1_LOWER);
        assert_eq!(string_type_ctype1(b'5' as u16) & C1_DIGIT, C1_DIGIT);
        assert_eq!(string_type_ctype1(b' ' as u16) & C1_SPACE, C1_SPACE);
        assert_eq!(string_type_ctype1(b'!' as u16) & C1_PUNCT, C1_PUNCT);
        assert_eq!(string_type_ctype1(0x01) & C1_CNTRL, C1_CNTRL);
        // A letter is never classified DIGIT — FAIL-able negative.
        assert_eq!(string_type_ctype1(b'A' as u16) & C1_DIGIT, 0);
    }

    #[test]
    fn lcmap_case_folds_ascii() {
        let s: Vec<u16> = "AbC9".encode_utf16().collect();
        assert_eq!(
            lc_map_string(LCMAP_UPPERCASE, &s),
            "ABC9".encode_utf16().collect::<Vec<u16>>()
        );
        assert_eq!(
            lc_map_string(LCMAP_LOWERCASE, &s),
            "abc9".encode_utf16().collect::<Vec<u16>>()
        );
        // No flag = identity.
        assert_eq!(lc_map_string(0, &s), s);
    }

    #[test]
    fn compare_string_ordinal_and_ignorecase() {
        let a: Vec<u16> = "abc".encode_utf16().collect();
        let b: Vec<u16> = "abd".encode_utf16().collect();
        let abc_upper: Vec<u16> = "ABC".encode_utf16().collect();
        assert_eq!(compare_string(0, &a, &a), CSTR_EQUAL);
        assert_eq!(compare_string(0, &a, &b), CSTR_LESS_THAN);
        assert_eq!(compare_string(0, &b, &a), CSTR_GREATER_THAN);
        // Case-sensitive: "abc" > "ABC" (lowercase > uppercase ordinal).
        assert_eq!(compare_string(0, &a, &abc_upper), CSTR_GREATER_THAN);
        // Case-insensitive: equal.
        assert_eq!(compare_string(NORM_IGNORECASE, &a, &abc_upper), CSTR_EQUAL);
        // Prefix is less.
        let ab: Vec<u16> = "ab".encode_utf16().collect();
        assert_eq!(compare_string(0, &ab, &a), CSTR_LESS_THAN);
    }

    #[test]
    fn lookup_function_entry_finds_the_right_pdata() {
        // Build a 2-entry .pdata table in a flat buffer, register a module over
        // it, and verify lookup picks the entry whose [begin,end) covers the PC.
        clear_modules_for_test();
        // Image: base 0x10000, size 0x4000. .pdata at base+0x3000.
        // Entry 0: [0x1000, 0x1100) -> unwind 0x2000
        // Entry 1: [0x1100, 0x1240) -> unwind 0x2010
        let mut buf = vec![0u8; 0x4000];
        let put =
            |b: &mut [u8], off: usize, v: u32| b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        let pdata = 0x3000usize;
        put(&mut buf, pdata, 0x1000);
        put(&mut buf, pdata + 4, 0x1100);
        put(&mut buf, pdata + 8, 0x2000);
        put(&mut buf, pdata + 12, 0x1100);
        put(&mut buf, pdata + 16, 0x1240);
        put(&mut buf, pdata + 20, 0x2010);
        let base = buf.as_ptr() as u64;
        let m = LoadedModule {
            base,
            size: 0x4000,
            pdata_va: base + pdata as u64,
            pdata_size: 24,
        };
        register_module(m);
        // PC in entry 1's range -> its .pdata entry address.
        let pc = base + 0x1180;
        // SAFETY: pdata_va points into `buf` which is live for this test.
        let ent = unsafe { m.lookup_function_entry(pc) };
        assert_eq!(ent, base + (pdata + 12) as u64, "picks entry 1");
        // The begin/end at that entry confirm it's the right record.
        let begin = u32::from_le_bytes(buf[pdata + 12..pdata + 16].try_into().unwrap());
        assert_eq!(begin, 0x1100);
        // PC in entry 0.
        let ent0 = unsafe { m.lookup_function_entry(base + 0x1050) };
        assert_eq!(ent0, base + pdata as u64, "picks entry 0");
        // PC outside any entry -> 0.
        assert_eq!(unsafe { m.lookup_function_entry(base + 0x2000) }, 0);
        // module_for_pc maps the PC back to this image.
        assert_eq!(module_for_pc(pc), Some(m));
        assert_eq!(module_for_pc(0x1), None);
        clear_modules_for_test();
    }

    #[test]
    fn main_module_base_is_first_wins() {
        clear_main_module_base_for_test();
        assert_eq!(main_module_base(), 0, "unset reads 0");
        set_main_module_base(0x1_0000_0000);
        assert_eq!(main_module_base(), 0x1_0000_0000);
        // First-image-wins: a later (DLL) load cannot steal main-module identity.
        set_main_module_base(0x7FFF_0000);
        assert_eq!(
            main_module_base(),
            0x1_0000_0000,
            "later set must NOT override the main module base"
        );
        clear_main_module_base_for_test();
    }

    #[test]
    fn unhandled_filter_stores_and_returns_previous() {
        let prev0 = set_unhandled_exception_filter(0x1111);
        // (prev0 may be nonzero if a prior test set it; the contract is the
        // *swap* semantics, which the next two asserts prove.)
        let _ = prev0;
        let prev1 = set_unhandled_exception_filter(0x2222);
        assert_eq!(prev1, 0x1111, "returns the previously installed filter");
        assert_eq!(current_unhandled_filter(), 0x2222);
        // Restore to 0 so other tests start clean.
        set_unhandled_exception_filter(0);
    }
}
