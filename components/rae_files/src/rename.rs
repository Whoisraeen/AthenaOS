//! Batch rename — Finder-style. A pattern such as `Photo_###` expands per
//! selected file with a zero-padded counter (`###` → width-3), and the original
//! **extension is preserved** so `IMG2.png` and `IMG2.jpg` keep their types.
//! Collision-safe by construction at the logic layer (two distinct indices never
//! produce the same name); the app still handles `E_VFS_EXISTS` from
//! `SYS_RENAME` against pre-existing on-disk files.
//!
//! Pure string arithmetic over fixed buffers — no `alloc`, no panics.

use crate::PATH_CAP;

/// Max length of a single produced file name (name + extension).
pub const MAX_NAME: usize = 96;

/// Why an expansion failed. The UI shows the matching calm message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameError {
    /// The pattern contains no `#` counter run — a batch rename of >1 file with
    /// no counter would collide every name, so it is rejected up front.
    NoCounter,
    /// The pattern (or a produced name) is empty after trimming.
    Empty,
    /// The produced name would exceed [`MAX_NAME`] or the path [`PATH_CAP`].
    TooLong,
    /// The counter run is wider than we format (cap 9 digits).
    CounterTooWide,
    /// The pattern contains a path separator (`/`) — names cannot span dirs.
    HasSeparator,
}

/// Split a file name into `(stem, ext_with_dot)`. A leading dot (dotfile) is part
/// of the stem, not an extension: `".raeconfig"` → `(".raeconfig", "")`. No
/// extension → `("README", "")`.
pub fn split_name_ext(name: &str) -> (&str, &str) {
    let b = name.as_bytes();
    // Find the LAST '.' that is not at index 0.
    let mut dot: Option<usize> = None;
    for (i, &c) in b.iter().enumerate() {
        if c == b'.' && i != 0 {
            dot = Some(i);
        }
    }
    match dot {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    }
}

/// A fixed-capacity produced name (alloc-free).
#[derive(Clone, Copy)]
pub struct Name {
    buf: [u8; MAX_NAME],
    len: usize,
}

impl Name {
    fn new() -> Self {
        Self {
            buf: [0; MAX_NAME],
            len: 0,
        }
    }
    fn push_bytes(&mut self, b: &[u8]) -> bool {
        if self.len + b.len() > MAX_NAME {
            return false;
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        true
    }
    fn push_str(&mut self, s: &str) -> bool {
        self.push_bytes(s.as_bytes())
    }
    /// The produced name as a `&str`.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Debug for Name {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Name({:?})", self.as_str())
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for Name {}

/// Expand `pattern` for item `index` (0-based; the displayed counter is
/// `index + start`). A run of `#` is the counter, zero-padded to the run width.
/// Everything else is literal. Example: `expand_pattern("Photo_###", 0, 1)` →
/// `"Photo_001"`. The extension is NOT added here (see [`batch_rename_target`]).
pub fn expand_pattern(pattern: &str, index: u32, start: u32) -> Result<Name, RenameError> {
    if pattern.is_empty() {
        return Err(RenameError::Empty);
    }
    if pattern.contains('/') {
        return Err(RenameError::HasSeparator);
    }

    let bytes = pattern.as_bytes();
    let mut out = Name::new();
    let mut saw_counter = false;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'#' {
            // Count the run width.
            let run_start = i;
            while i < bytes.len() && bytes[i] == b'#' {
                i += 1;
            }
            let width = i - run_start;
            if width > 9 {
                return Err(RenameError::CounterTooWide);
            }
            saw_counter = true;
            let value = (index as u64) + (start as u64);
            if !write_padded(&mut out, value, width) {
                return Err(RenameError::TooLong);
            }
        } else {
            // Copy this literal byte.
            if !out.push_bytes(&bytes[i..i + 1]) {
                return Err(RenameError::TooLong);
            }
            i += 1;
        }
    }

    if !saw_counter {
        return Err(RenameError::NoCounter);
    }
    if out.len == 0 {
        return Err(RenameError::Empty);
    }
    Ok(out)
}

/// Write `value` zero-padded to at least `width` digits into `out`.
fn write_padded(out: &mut Name, value: u64, width: usize) -> bool {
    let mut tmp = [0u8; 20];
    let mut n = 0;
    let mut v = value;
    if v == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    // Leading zeros to reach `width`.
    let pad = width.saturating_sub(n);
    for _ in 0..pad {
        if !out.push_bytes(b"0") {
            return false;
        }
    }
    // Reverse `tmp` into `out`.
    while n > 0 {
        n -= 1;
        if !out.push_bytes(&tmp[n..n + 1]) {
            return false;
        }
    }
    true
}

/// The full produced file name for `original` under `pattern` at `index`:
/// `expand_pattern(...)` + the original's preserved extension. This is the
/// `new` name the app passes to `SYS_RENAME` (joined onto the parent dir).
///
/// `original` is the current file name (e.g. `"vacation.jpg"`); `parent_len` is
/// the length of the parent dir path so we can reject results that would overflow
/// the full VFS path (`parent_len + 1 + name`).
pub fn batch_rename_target(
    original: &str,
    pattern: &str,
    index: u32,
    start: u32,
    parent_len: usize,
) -> Result<Name, RenameError> {
    let (_stem, ext) = split_name_ext(original);
    let mut name = expand_pattern(pattern, index, start)?;
    if !name.push_str(ext) {
        return Err(RenameError::TooLong);
    }
    // Guard the eventual joined path `<parent>/<name>` against PATH_CAP.
    if parent_len + 1 + name.len > PATH_CAP {
        return Err(RenameError::TooLong);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_basic() {
        assert_eq!(split_name_ext("vacation.jpg"), ("vacation", ".jpg"));
        assert_eq!(split_name_ext("archive.tar.gz"), ("archive.tar", ".gz"));
        assert_eq!(split_name_ext("README"), ("README", ""));
        assert_eq!(split_name_ext(".raeconfig"), (".raeconfig", "")); // dotfile
    }

    #[test]
    fn expand_pads_counter() {
        assert_eq!(
            expand_pattern("Photo_###", 0, 1).unwrap().as_str(),
            "Photo_001"
        );
        assert_eq!(
            expand_pattern("Photo_###", 41, 1).unwrap().as_str(),
            "Photo_042"
        );
        assert_eq!(
            expand_pattern("Photo_###", 998, 1).unwrap().as_str(),
            "Photo_999"
        );
        // Overflowing the width keeps full precision (no truncation).
        assert_eq!(expand_pattern("x#", 41, 1).unwrap().as_str(), "x42");
    }

    #[test]
    fn counter_can_appear_anywhere() {
        assert_eq!(
            expand_pattern("###-final", 0, 1).unwrap().as_str(),
            "001-final"
        );
        assert_eq!(expand_pattern("a##b##", 0, 5).unwrap().as_str(), "a05b05");
    }

    #[test]
    fn no_counter_is_rejected() {
        // A counter-less pattern would collide every name — must error, not
        // silently produce duplicates.
        assert_eq!(expand_pattern("Photo", 0, 1), Err(RenameError::NoCounter));
    }

    #[test]
    fn separator_is_rejected() {
        assert_eq!(
            expand_pattern("a/b###", 0, 1),
            Err(RenameError::HasSeparator)
        );
    }

    #[test]
    fn target_preserves_extension() {
        let n = batch_rename_target("vacation.jpg", "Photo_###", 0, 1, 20).unwrap();
        assert_eq!(n.as_str(), "Photo_001.jpg");
        let n2 = batch_rename_target("clip.tar.gz", "File_##", 9, 1, 20).unwrap();
        assert_eq!(n2.as_str(), "File_10.gz");
    }

    #[test]
    fn extensionless_target_has_no_dot() {
        let n = batch_rename_target("README", "Doc_##", 0, 1, 20).unwrap();
        assert_eq!(n.as_str(), "Doc_01");
    }

    #[test]
    fn over_long_path_is_rejected() {
        // parent_len near PATH_CAP forces TooLong even for a short name.
        assert_eq!(
            batch_rename_target("a.txt", "n_###", 0, 1, PATH_CAP - 2),
            Err(RenameError::TooLong)
        );
    }

    #[test]
    fn distinct_indices_never_collide() {
        // The core collision-freedom guarantee that makes batch rename safe.
        let a = batch_rename_target("a.png", "P_###", 0, 1, 20).unwrap();
        let b = batch_rename_target("b.png", "P_###", 1, 1, 20).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn fail_able_wrong_expansion_is_caught() {
        // FAIL-able: if the padding logic regressed to no-pad or off-by-one this
        // assert_ne (paired with the assert_eq above) pins the exact output.
        let n = expand_pattern("Photo_###", 0, 1).unwrap();
        assert_ne!(n.as_str(), "Photo_0"); // must be width-3 padded
        assert_ne!(n.as_str(), "Photo_000"); // counter starts at `start`, not 0
        assert_ne!(n.as_str(), "Photo_1"); // must be padded
    }
}
