//! Application path resolution — Concept §Compatibility Strategy:
//! apps install under `/system/apps/<name>` with flat initramfs names as fallback.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Build an ordered list of path strings to try when opening an executable.
pub fn resolve_candidates(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return out;
    }

    let push_unique = |out: &mut Vec<String>, s: &str| {
        if s.is_empty() {
            return;
        }
        if !out.iter().any(|x| x == s) {
            out.push(String::from(s));
        }
    };

    push_unique(&mut out, trimmed);

    let basename = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if basename != trimmed {
        push_unique(&mut out, basename);
    }

    if !trimmed.starts_with("/system/apps/") {
        push_unique(&mut out, &alloc::format!("/system/apps/{}", basename));
        push_unique(&mut out, &alloc::format!("system/apps/{}", basename));
    }

    out
}
