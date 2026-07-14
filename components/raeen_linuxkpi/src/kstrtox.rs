//! `kstrto*` — strict string→integer parsing (sysfs / debugfs / module params).
//!
//! When a driver exposes a writable sysfs/debugfs knob, the core hands the
//! written bytes to `kstrtoul`/`kstrtoint`/… to parse — and crucially to REJECT
//! garbage (`-EINVAL`) and overflow (`-ERANGE`). A lax parser would silently
//! accept bad input and mis-program the hardware, so the shim mirrors Linux's
//! exact contract: optional sign, `0x`/`0` radix auto-detect on base 0, one
//! trailing newline allowed, and nothing else after the digits.
//!
//! Pure logic over a NUL-terminated C string — host-KAT-able directly through
//! the real `extern "C"` entry points (they are not variadic).

const EINVAL: i32 = -22;
const ERANGE: i32 = -34;

#[inline]
fn digit_val(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'z' => Some((c - b'a' + 10) as u32),
        b'A'..=b'Z' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// NUL-terminated C string as a slice (capped — these are short knob values).
#[inline]
unsafe fn cstr<'a>(s: *const u8) -> &'a [u8] {
    if s.is_null() {
        return &[];
    }
    let mut n = 0;
    while n < 256 && *s.add(n) != 0 {
        n += 1;
    }
    core::slice::from_raw_parts(s, n)
}

/// Parse an unsigned integer body (no leading sign). Mirrors Linux
/// `_kstrtoull`: radix fixup, digit run, optional single trailing `\n`, then
/// end-of-string. Returns the value or a negative errno.
pub(crate) fn parse_ull(s: &[u8], mut base: u32) -> Result<u64, i32> {
    let mut i = 0usize;
    if base == 0 {
        if s.first() == Some(&b'0') {
            let x_next = matches!(s.get(1), Some(&b'x') | Some(&b'X'));
            let xdigit = s.get(2).and_then(|&c| digit_val(c)).is_some_and(|d| d < 16);
            if x_next && xdigit {
                base = 16;
                i = 2;
            } else {
                base = 8; // leading 0 stays a digit (parses to the same value)
            }
        } else {
            base = 10;
        }
    } else if base == 16
        && s.first() == Some(&b'0')
        && matches!(s.get(1), Some(&b'x') | Some(&b'X'))
    {
        i = 2;
    }
    let mut val: u64 = 0;
    let mut any = false;
    while let Some(&c) = s.get(i) {
        match digit_val(c) {
            Some(d) if d < base => {
                val = val
                    .checked_mul(base as u64)
                    .and_then(|v| v.checked_add(d as u64))
                    .ok_or(ERANGE)?;
                any = true;
                i += 1;
            }
            _ => break,
        }
    }
    if !any {
        return Err(EINVAL);
    }
    if s.get(i) == Some(&b'\n') {
        i += 1;
    }
    if i != s.len() {
        return Err(EINVAL);
    }
    Ok(val)
}

/// `kstrtoull(s, base, res)`.
#[no_mangle]
pub unsafe extern "C" fn kstrtoull(s: *const u8, base: u32, res: *mut u64) -> i32 {
    let mut b = cstr(s);
    if b.first() == Some(&b'+') {
        b = &b[1..];
    }
    match parse_ull(b, base) {
        Ok(v) => {
            if !res.is_null() {
                *res = v;
            }
            0
        }
        Err(e) => e,
    }
}

/// `kstrtoll(s, base, res)` — signed.
#[no_mangle]
pub unsafe extern "C" fn kstrtoll(s: *const u8, base: u32, res: *mut i64) -> i32 {
    let b = cstr(s);
    let (neg, body): (bool, &[u8]) = match b.first() {
        Some(&b'-') => (true, &b[1..]),
        Some(&b'+') => (false, &b[1..]),
        _ => (false, b),
    };
    let v = match parse_ull(body, base) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // i128 keeps the i64::MIN (= -(2^63)) edge case exact.
    let signed = if neg { -(v as i128) } else { v as i128 };
    if signed < i64::MIN as i128 || signed > i64::MAX as i128 {
        return ERANGE;
    }
    if !res.is_null() {
        *res = signed as i64;
    }
    0
}

/// `kstrtouint(s, base, res)`.
#[no_mangle]
pub unsafe extern "C" fn kstrtouint(s: *const u8, base: u32, res: *mut u32) -> i32 {
    let mut tmp: u64 = 0;
    let rv = kstrtoull(s, base, &mut tmp);
    if rv != 0 {
        return rv;
    }
    if tmp > u32::MAX as u64 {
        return ERANGE;
    }
    if !res.is_null() {
        *res = tmp as u32;
    }
    0
}

/// `kstrtoint(s, base, res)`.
#[no_mangle]
pub unsafe extern "C" fn kstrtoint(s: *const u8, base: u32, res: *mut i32) -> i32 {
    let mut tmp: i64 = 0;
    let rv = kstrtoll(s, base, &mut tmp);
    if rv != 0 {
        return rv;
    }
    if tmp < i32::MIN as i64 || tmp > i32::MAX as i64 {
        return ERANGE;
    }
    if !res.is_null() {
        *res = tmp as i32;
    }
    0
}

/// `kstrtol(s, base, res)` — signed long. On LP64 `long` is 64-bit, so this is
/// exactly `kstrtoll`.
#[no_mangle]
pub unsafe extern "C" fn kstrtol(s: *const u8, base: u32, res: *mut i64) -> i32 {
    kstrtoll(s, base, res)
}
/// `kstrtoul(s, base, res)` — unsigned long (== `kstrtoull` on LP64).
#[no_mangle]
pub unsafe extern "C" fn kstrtoul(s: *const u8, base: u32, res: *mut u64) -> i32 {
    kstrtoull(s, base, res)
}

/// `kstrtouint_from_user(s, count, base, res)` — parse a counted user buffer
/// (in the daemon, ordinary memory). NUL-terminates a bounded copy, then parses.
#[no_mangle]
pub unsafe extern "C" fn kstrtouint_from_user(
    s: *const u8,
    count: usize,
    base: u32,
    res: *mut u32,
) -> i32 {
    let mut buf = [0u8; 64];
    let n = count.min(buf.len() - 1);
    if !s.is_null() {
        core::ptr::copy_nonoverlapping(s, buf.as_mut_ptr(), n);
    }
    buf[n] = 0;
    kstrtouint(buf.as_ptr(), base, res)
}
