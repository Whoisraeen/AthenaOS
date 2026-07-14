//! Shared text helpers for RaeShell render paths.
//!
//! Every shell surface that truncates a label to fit a pixel/glyph budget used
//! to do `&s[..N]` — a raw BYTE slice. That PANICS the instant byte `N` lands
//! inside a multi-byte UTF-8 code point, which happens for any accented
//! filename, CJK window title, or emoji in a now-playing track name. This
//! module owns the one boundary-safe truncation primitive so no surface has to
//! re-derive it (and so the bug can never reappear in a new copy).
//!
//! Concept: *"Built for people who care about how things feel."* — a shell that
//! crashes on a `Café` folder or a `文档` window is not that shell.

/// Truncate a `&str` to at most `max_chars` Unicode scalar values, returning a
/// borrowed prefix that always ends on a UTF-8 character boundary.
///
/// Shell render code renders arbitrary content — file paths, window titles,
/// track names, notification bodies — that can contain multi-byte code points
/// (accents, CJK, emoji). A naive `&s[..N]` byte slice PANICS when byte `N`
/// lands inside a code point; this helper finds the byte offset of the
/// `max_chars`-th character and slices there, so it can never split a glyph or
/// panic. When the string already has `<= max_chars` characters it is returned
/// whole.
#[inline]
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s, // fewer than `max_chars` chars — whole string is safe
    }
}

// ── Host KAT (R10: a smoketest must be able to print FAIL) ─────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn truncate_chars_never_splits_multibyte() {
        // The exact panic class: a label holding accented / CJK / emoji code
        // points. A raw byte slice `&s[..N]` would panic when N lands inside one
        // of these; `truncate_chars` must not.
        let s = "Café_文档_🎮_a_very_long_label_string_to_force_truncation_xyz";
        for n in 0..=s.chars().count() + 4 {
            let out = truncate_chars(s, n);
            // 1) Must be a valid prefix (== the first `n` chars of the source).
            let expect: String = s.chars().take(n).collect();
            assert_eq!(out, expect, "truncate_chars({n}) returned wrong prefix");
            // 2) Must end on a char boundary (re-slicing at out.len() is valid).
            assert!(
                s.is_char_boundary(out.len()),
                "truncate_chars({n}) ended mid-codepoint at byte {}",
                out.len()
            );
        }

        // The EXACT original panic: a fixed-byte cut that lands mid-codepoint.
        // 63 ASCII + a 2-byte 'é' puts char #64 starting at byte 63 and ending
        // at byte 65 — `&label[..64]` would panic here; we must not.
        let mut bug = "a".repeat(63);
        bug.push('é');
        bug.push_str("trailing content to push total length past 64 chars ok ok");
        let cut = truncate_chars(&bug, 64);
        assert_eq!(cut.chars().count(), 64, "should keep exactly 64 chars");
        assert_eq!(&cut[63..], "é", "the 64th char must be kept whole");

        // Pure multibyte run: 100×'é' = 200 bytes; byte 64 is mid-codepoint.
        let long = "é".repeat(100);
        let cut = truncate_chars(&long, 64);
        assert_eq!(cut.chars().count(), 64, "should keep exactly 64 chars");
        assert!(long.is_char_boundary(cut.len()));

        // An emoji (4-byte) at the cut: a single 🎮 spanning the boundary.
        let emoji = "ab🎮cd🎮ef🎮gh";
        for n in 0..=emoji.chars().count() {
            let out = truncate_chars(emoji, n);
            assert!(emoji.is_char_boundary(out.len()));
            assert_eq!(out.chars().count(), n.min(emoji.chars().count()));
        }
    }

    #[test]
    fn truncate_chars_passes_through_short_strings() {
        assert_eq!(truncate_chars("hi", 64), "hi");
        assert_eq!(truncate_chars("", 4), "");
        assert_eq!(truncate_chars("abc", 0), "");
    }
}
