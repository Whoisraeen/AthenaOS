//! # RaeDiff — a never-panic, `no_std` diff/patch engine.
//!
//! LEGACY_GAMING_CONCEPT.md §"atomic CoW updates + one-click rollback": an OS update
//! should ship **only the changed bytes**, not a full image, then land atomically
//! into the inactive A/B slot. The byte-delta primitive here ([`byte_delta`] /
//! [`apply_delta`]) is exactly that on-the-wire update format: a `Copy{src,len}`
//! from the old slot's contents plus literal `Data` for the new bytes, so a
//! delta update transfers and stores only the difference between the two system
//! images.
//!
//! The same engine powers **editor change-views and version-control**: the line
//! diff ([`diff_lines`]) and the standard `unified_diff`/`parse_unified`/`apply`
//! triple (the `@@ -a,b +c,d @@` format `patch` and git speak) give a text-side
//! diff/patch round-trip a code editor or a config-rollback UI can render and
//! apply.
//!
//! This crate is foundational, dependency-free infrastructure and is deliberately
//! wired into no consumer this slice — a delta-update follow-up wires
//! [`byte_delta`] into the update daemon to ship deltas instead of full slots.
//!
//! ## Never-panic posture (CLAUDE §9-class safety: update code is dangerous)
//! Every input is treated as hostile. There is **no `unwrap`/`expect`/`panic`/
//! raw-index-panic path** reachable from any public function: a patch whose
//! context does not match `old`, a malformed `@@` header, a truncated patch, or
//! garbage bytes all return `Err(_)`; empty and large inputs are handled
//! gracefully. The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_diff`), including a malformed-input battery that asserts
//! zero panics.
//!
//! ## Algorithms
//! - [`diff_lines`]: the Myers O(ND) shortest-edit-script algorithm over lines,
//!   producing a correct (applying it to `old` yields `new`) edit script of
//!   [`DiffOp`]s.
//! - [`unified_diff`]: the standard unified-diff text format with `@@` hunk
//!   headers and ` `/`-`/`+` lines; [`parse_unified`] + [`apply`] round-trip it.
//! - [`byte_delta`]: a common-prefix/common-suffix + literal-middle binary delta
//!   (documented as a first-cut; rolling-hash block matching is a future bonus),
//!   with [`apply_delta`] reconstructing `new` exactly.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// A single operation in a line-level edit script produced by [`diff_lines`].
///
/// `Equal`/`Delete` carry *index ranges into the old line array*; `Insert`
/// carries the literal new lines. Applying the whole script to `old`'s lines in
/// order reconstructs `new`'s lines exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOp {
    /// Lines `old[start..start+len]` are unchanged (also present in `new`).
    Equal { start: usize, len: usize },
    /// Lines `old[start..start+len]` are removed.
    Delete { start: usize, len: usize },
    /// These literal lines are inserted (present in `new`, absent from `old`).
    Insert { lines: Vec<String> },
}

/// Errors returned by the parse/apply paths. No public function panics; every
/// failure is one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// A `@@ -a,b +c,d @@` hunk header was missing or malformed.
    BadHunkHeader,
    /// A hunk body line did not start with ` `, `-`, or `+`.
    BadHunkLine,
    /// The patch's context/removed lines did not match `old` at the hunk
    /// location — the patch does not apply to this input.
    ContextMismatch,
    /// A hunk's declared line counts did not match its body (truncated patch).
    Truncated,
    /// A byte-delta `Copy` referenced a range outside `old`.
    DeltaOutOfRange,
}

// ===========================================================================
// 1. Line diff — Myers O(ND) shortest edit script.
// ===========================================================================

/// Split text into lines, *keeping* the trailing newline on each line so the
/// diff is exact for the byte content (a final line without a newline is kept as
/// its own element). An empty string yields zero lines.
fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // include the '\n'
            out.push(&s[start..=i]);
            start = i + 1;
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&s[start..]);
    }
    out
}

/// Compute a correct (not necessarily provably minimal, but minimal-ish via
/// Myers) line-level edit script transforming `old` into `new`.
///
/// Applying the returned [`DiffOp`]s to `old`'s lines reproduces `new` exactly.
/// Never panics. Work is bounded by `(N+M)` diagonals × edit distance; for
/// pathological fully-disjoint inputs this is O((N+M)·D) which is bounded by
/// O((N+M)^2) — acceptable for source/config sizes.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffOp> {
    let a = split_lines(old);
    let b = split_lines(new);
    let trace = myers_trace(&a, &b);
    backtrack_to_ops(&a, &b, &trace)
}

/// One snapshot of the Myers `V` array at each edit-distance `d`. We record the
/// whole trace so we can backtrack to a concrete edit script.
fn myers_trace(a: &[&str], b: &[&str]) -> Vec<Vec<isize>> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max = (n + m) as usize;
    // V is indexed by diagonal k in [-(max+1), max+1]; offset by `max + 1` so the
    // `k±1` neighbour reads at the extremes (and the max==0 case, where the only
    // diagonal is k=0 but we still read k+1) are always in bounds.
    let offset = (max + 1) as isize;
    let mut v = vec![0isize; 2 * (max + 1) + 1];
    let mut trace: Vec<Vec<isize>> = Vec::new();

    // d from 0..=max. If a==b (max==0), the d==0 pass still runs and the
    // backtrack produces an all-equal script.
    let mut d = 0isize;
    while d <= max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            // choose to go down (insert) or right (delete)
            let idx = (k + offset) as usize;
            let down =
                k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]);
            let mut x = if down {
                v[(k + 1 + offset) as usize]
            } else {
                v[(k - 1 + offset) as usize] + 1
            };
            let mut y = x - k;
            // follow the diagonal (matching lines)
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                // reached the end; this d is the edit distance.
                return trace;
            }
            k += 2;
        }
        d += 1;
    }
    trace
}

/// Walk the recorded Myers trace backwards from `(n,m)` to `(0,0)`, emitting the
/// edit script in forward order. Coalesces consecutive ops of the same kind.
fn backtrack_to_ops(a: &[&str], b: &[&str], trace: &[Vec<isize>]) -> Vec<DiffOp> {
    let n = a.len() as isize;
    let m = b.len() as isize;
    let max = (n + m) as usize;
    let offset = (max + 1) as isize;

    // Collect reversed (x_prev,y_prev)->(x,y) segments, then reverse.
    // Each step is either a diagonal (equal), a "right" (delete), or "down"
    // (insert). We record per-line moves to coalesce afterwards.
    #[derive(Clone, Copy)]
    enum Step {
        Eq(usize),  // old index that is equal
        Del(usize), // old index deleted
        Ins(usize), // new index inserted
    }
    let mut steps_rev: Vec<Step> = Vec::new();

    let mut x = n;
    let mut y = m;
    // trace[d] is the V array *before* processing edit-distance d.
    let mut d = trace.len() as isize - 1;
    while d > 0 {
        let v = &trace[d as usize];
        let k = x - y;
        let down =
            k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]);
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = v[(prev_k + offset) as usize];
        let prev_y = prev_x - prev_k;

        // follow diagonal back down to (mid_x, mid_y)
        while x > prev_x && y > prev_y {
            // a[x-1]==b[y-1] equal line
            steps_rev.push(Step::Eq((x - 1) as usize));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if down {
                // an insertion: consumed b[prev_y]
                if y > 0 {
                    steps_rev.push(Step::Ins((y - 1) as usize));
                }
            } else {
                // a deletion: consumed a[prev_x]
                if x > 0 {
                    steps_rev.push(Step::Del((x - 1) as usize));
                }
            }
        }
        x = prev_x;
        y = prev_y;
        d -= 1;
    }
    // d == 0: remaining diagonal from (x,y) back to (0,0) is all equal.
    while x > 0 && y > 0 {
        steps_rev.push(Step::Eq((x - 1) as usize));
        x -= 1;
        y -= 1;
    }
    // Any residual pure inserts/deletes at the origin (when one side is empty).
    while y > 0 {
        steps_rev.push(Step::Ins((y - 1) as usize));
        y -= 1;
    }
    while x > 0 {
        steps_rev.push(Step::Del((x - 1) as usize));
        x -= 1;
    }

    // steps_rev is in reverse order; reverse to forward.
    steps_rev.reverse();

    // Coalesce into DiffOps.
    let mut ops: Vec<DiffOp> = Vec::new();
    for step in steps_rev {
        match step {
            Step::Eq(i) => match ops.last_mut() {
                Some(DiffOp::Equal { start, len }) if *start + *len == i => *len += 1,
                _ => ops.push(DiffOp::Equal { start: i, len: 1 }),
            },
            Step::Del(i) => match ops.last_mut() {
                Some(DiffOp::Delete { start, len }) if *start + *len == i => *len += 1,
                _ => ops.push(DiffOp::Delete { start: i, len: 1 }),
            },
            Step::Ins(j) => {
                let line = b[j].to_string();
                match ops.last_mut() {
                    Some(DiffOp::Insert { lines }) => lines.push(line),
                    _ => ops.push(DiffOp::Insert { lines: vec![line] }),
                }
            }
        }
    }
    ops
}

/// Apply a line-level edit script to `old`, returning the reconstructed text.
/// This is the round-trip partner of [`diff_lines`]. Returns `Err` if an op
/// references lines outside `old` (a corrupt script) rather than panicking.
pub fn apply_line_ops(old: &str, ops: &[DiffOp]) -> Result<String, DiffError> {
    let a = split_lines(old);
    let mut out = String::new();
    for op in ops {
        match op {
            DiffOp::Equal { start, len } => {
                let end = start.checked_add(*len).ok_or(DiffError::Truncated)?;
                if end > a.len() {
                    return Err(DiffError::ContextMismatch);
                }
                for line in &a[*start..end] {
                    out.push_str(line);
                }
            }
            DiffOp::Delete { start, len } => {
                let end = start.checked_add(*len).ok_or(DiffError::Truncated)?;
                if end > a.len() {
                    return Err(DiffError::ContextMismatch);
                }
                // deletions produce no output
            }
            DiffOp::Insert { lines } => {
                for line in lines {
                    out.push_str(line);
                }
            }
        }
    }
    Ok(out)
}

// ===========================================================================
// 2. Unified diff — text format + parse + apply.
// ===========================================================================

/// A parsed unified-diff hunk: the `@@ -old_start,old_len +new_start,new_len @@`
/// header (1-based line numbers as in the textual format) plus its body lines,
/// each tagged by leading marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    /// Body lines: `(Tag, content-without-trailing-newline-marker)`. The content
    /// keeps the original line text (including its own `\n` if it had one).
    pub lines: Vec<(Tag, String)>,
}

/// Tag for a unified-diff body line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// ` ` context (present in both old and new).
    Context,
    /// `-` removed (present in old only).
    Removed,
    /// `+` added (present in new only).
    Added,
}

/// A whole parsed patch: an ordered list of hunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub hunks: Vec<Hunk>,
}

/// Produce a standard unified diff of `old`→`new` with `context_lines` of
/// surrounding context per hunk. The output is the `@@ -a,b +c,d @@` format that
/// `patch` and git consume. Never panics.
pub fn unified_diff(old: &str, new: &str, context_lines: usize) -> String {
    let a = split_lines(old);
    let b = split_lines(new);
    let ops = diff_lines(old, new);

    // Flatten the edit script into per-(old,new) line tags so we can group hunks.
    // Each entry: (Tag, old_index option, new_index option).
    #[derive(Clone)]
    enum Tagged<'l> {
        Ctx(&'l str),
        Del(&'l str),
        Ins(&'l str),
    }
    let mut tagged: Vec<Tagged> = Vec::new();
    let mut ni = 0usize; // running new-line index for Insert ops
    for op in &ops {
        match op {
            DiffOp::Equal { start, len } => {
                for i in *start..*start + *len {
                    if i < a.len() {
                        tagged.push(Tagged::Ctx(a[i]));
                    }
                    ni += 1;
                }
            }
            DiffOp::Delete { start, len } => {
                for i in *start..*start + *len {
                    if i < a.len() {
                        tagged.push(Tagged::Del(a[i]));
                    }
                }
            }
            DiffOp::Insert { lines } => {
                for _ in lines {
                    if ni < b.len() {
                        tagged.push(Tagged::Ins(b[ni]));
                    }
                    ni += 1;
                }
            }
        }
    }

    // Identify changed positions; group into hunks with `context_lines` context.
    let is_change: Vec<bool> = tagged
        .iter()
        .map(|t| !matches!(t, Tagged::Ctx(_)))
        .collect();
    if !is_change.iter().any(|c| *c) {
        return String::new(); // no differences
    }

    let mut out = String::new();
    let n = tagged.len();
    let mut idx = 0usize;
    while idx < n {
        if !is_change[idx] {
            idx += 1;
            continue;
        }
        // Found a change. Expand hunk to include context before/after, merging
        // adjacent changes separated by <= 2*context context lines.
        let hunk_start = idx.saturating_sub(context_lines);
        let mut hunk_end = idx;
        let mut j = idx;
        while j < n {
            if is_change[j] {
                hunk_end = j;
                j += 1;
            } else {
                // count trailing context run; stop if it exceeds the merge gap.
                let mut k = j;
                while k < n && !is_change[k] {
                    k += 1;
                }
                let gap = k - j;
                if k < n && gap <= context_lines * 2 {
                    // bridge: include this context and continue
                    j = k;
                } else {
                    break;
                }
            }
        }
        let hunk_tail = (hunk_end + 1 + context_lines).min(n);

        // Compute 1-based start lines and counts.
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        // old/new starts = 1 + number of old/new lines before hunk_start.
        let mut old_before = 0usize;
        let mut new_before = 0usize;
        for t in &tagged[..hunk_start] {
            match t {
                Tagged::Ctx(_) => {
                    old_before += 1;
                    new_before += 1;
                }
                Tagged::Del(_) => old_before += 1,
                Tagged::Ins(_) => new_before += 1,
            }
        }
        let mut body = String::new();
        for t in &tagged[hunk_start..hunk_tail] {
            match t {
                Tagged::Ctx(l) => {
                    old_count += 1;
                    new_count += 1;
                    push_body_line(&mut body, ' ', l);
                }
                Tagged::Del(l) => {
                    old_count += 1;
                    push_body_line(&mut body, '-', l);
                }
                Tagged::Ins(l) => {
                    new_count += 1;
                    push_body_line(&mut body, '+', l);
                }
            }
        }
        let old_start = if old_count == 0 {
            old_before
        } else {
            old_before + 1
        };
        let new_start = if new_count == 0 {
            new_before
        } else {
            new_before + 1
        };

        out.push_str("@@ -");
        push_usize(&mut out, old_start);
        out.push(',');
        push_usize(&mut out, old_count);
        out.push_str(" +");
        push_usize(&mut out, new_start);
        out.push(',');
        push_usize(&mut out, new_count);
        out.push_str(" @@\n");
        out.push_str(&body);

        idx = hunk_tail;
    }
    out
}

/// Append one body line with its marker. The content keeps its own trailing
/// `\n` if it had one. A line *without* a trailing newline (the last line of a
/// file with no final newline) is rendered as the content + a `\n` line
/// terminator followed by the standard `\ No newline at end of file` marker, so
/// the textual format stays line-oriented while losslessly recording the absence
/// of the final newline.
fn push_body_line(out: &mut String, marker: char, content: &str) {
    out.push(marker);
    out.push_str(content);
    if content.ends_with('\n') {
        // already a complete line
    } else {
        out.push('\n');
        out.push_str("\\ No newline at end of file\n");
    }
}

/// Append a `usize` in base-10 without `core::fmt` allocation surprises.
fn push_usize(out: &mut String, mut v: usize) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    // SAFETY-free: buf[i..] is ASCII digits by construction.
    for &d in &buf[i..] {
        out.push(d as char);
    }
}

/// Parse a unified-diff string into a [`Patch`]. Tolerates and ignores file
/// header lines (`--- `, `+++ `, `diff `, `index `) before the first `@@`.
/// Returns `Err` for a malformed `@@` header, a bad body line, or a truncated
/// hunk — never panics.
pub fn parse_unified(s: &str) -> Result<Patch, DiffError> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut cur: Option<Hunk> = None;
    let mut seen_old = 0usize;
    let mut seen_new = 0usize;

    for raw in s.split_inclusive('\n') {
        // strip trailing '\n' for marker inspection but keep it for content.
        let line = raw;
        if line.starts_with("@@") {
            // finish previous hunk
            if let Some(h) = cur.take() {
                if seen_old != h.old_len || seen_new != h.new_len {
                    return Err(DiffError::Truncated);
                }
                hunks.push(h);
            }
            let header = parse_hunk_header(line)?;
            seen_old = 0;
            seen_new = 0;
            cur = Some(header);
        } else if let Some(h) = cur.as_mut() {
            // body line; ignore a trailing empty split element
            if line.is_empty() {
                continue;
            }
            // The marker byte is always ASCII (' ', '+', '-', '\'), so slicing
            // off one byte for `content` is char-boundary-safe ONLY after the
            // marker has matched. A body line whose first char is a multibyte
            // codepoint has no valid marker and must return BadHunkLine, never
            // panic — so the `&line[1..]` slice lives inside each ASCII arm.
            let marker = line.as_bytes()[0];
            match marker {
                b' ' => {
                    h.lines.push((Tag::Context, line[1..].to_string()));
                    seen_old += 1;
                    seen_new += 1;
                }
                b'-' => {
                    h.lines.push((Tag::Removed, line[1..].to_string()));
                    seen_old += 1;
                }
                b'+' => {
                    h.lines.push((Tag::Added, line[1..].to_string()));
                    seen_new += 1;
                }
                b'\\' => {
                    // "\ No newline at end of file": the preceding body line's
                    // content was emitted with a synthetic `\n` line terminator;
                    // strip it so the stored content reflects the true (no final
                    // newline) bytes.
                    if let Some((_, prev)) = h.lines.last_mut() {
                        if prev.ends_with('\n') {
                            prev.pop();
                        }
                    }
                }
                _ => return Err(DiffError::BadHunkLine),
            }
        } else {
            // pre-hunk header lines (---, +++, diff, index, etc.): ignore.
            continue;
        }
    }
    if let Some(h) = cur.take() {
        if seen_old != h.old_len || seen_new != h.new_len {
            return Err(DiffError::Truncated);
        }
        hunks.push(h);
    }
    Ok(Patch { hunks })
}

/// Parse a single `@@ -a,b +c,d @@` header line. `b`/`d` default to 1 if omitted
/// (`@@ -a +c @@`). Returns `Err(BadHunkHeader)` on any deviation.
fn parse_hunk_header(line: &str) -> Result<Hunk, DiffError> {
    // Expected: @@ -OLD +NEW @@ [optional trailing section text]
    let line = line.trim_end_matches('\n');
    let rest = line.strip_prefix("@@ ").ok_or(DiffError::BadHunkHeader)?;
    // split off the closing " @@"
    let close = rest.find(" @@").ok_or(DiffError::BadHunkHeader)?;
    let spec = &rest[..close];
    // spec = "-a,b +c,d"
    let mut parts = spec.split(' ');
    let old_part = parts.next().ok_or(DiffError::BadHunkHeader)?;
    let new_part = parts.next().ok_or(DiffError::BadHunkHeader)?;
    if parts.next().is_some() {
        return Err(DiffError::BadHunkHeader);
    }
    let old = old_part.strip_prefix('-').ok_or(DiffError::BadHunkHeader)?;
    let new = new_part.strip_prefix('+').ok_or(DiffError::BadHunkHeader)?;
    let (old_start, old_len) = parse_range(old)?;
    let (new_start, new_len) = parse_range(new)?;
    Ok(Hunk {
        old_start,
        old_len,
        new_start,
        new_len,
        lines: Vec::new(),
    })
}

/// Parse `a,b` or `a` (len defaults to 1) into `(start, len)`.
fn parse_range(s: &str) -> Result<(usize, usize), DiffError> {
    match s.split_once(',') {
        Some((a, b)) => {
            let start = parse_usize(a)?;
            let len = parse_usize(b)?;
            Ok((start, len))
        }
        None => {
            let start = parse_usize(s)?;
            Ok((start, 1))
        }
    }
}

/// Parse a base-10 `usize`, rejecting empty/non-digit/overflow input.
fn parse_usize(s: &str) -> Result<usize, DiffError> {
    if s.is_empty() {
        return Err(DiffError::BadHunkHeader);
    }
    let mut v: usize = 0;
    for &c in s.as_bytes() {
        if !c.is_ascii_digit() {
            return Err(DiffError::BadHunkHeader);
        }
        v = v
            .checked_mul(10)
            .and_then(|x| x.checked_add((c - b'0') as usize))
            .ok_or(DiffError::BadHunkHeader)?;
    }
    Ok(v)
}

/// Apply a parsed [`Patch`] to `old`, returning the patched text. Verifies every
/// context and removed line matches `old` at the hunk's location; a mismatch
/// (the patch does not apply) returns `Err(ContextMismatch)`. Never panics.
pub fn apply(old: &str, patch: &Patch) -> Result<String, DiffError> {
    let a = split_lines(old);
    let mut out = String::new();
    // `cursor` is the next un-emitted old line index (0-based).
    let mut cursor = 0usize;

    for hunk in &patch.hunks {
        // old_start is 1-based; for a pure-insert hunk old_len==0 and old_start
        // is the line after which to insert (textual convention). Convert to a
        // 0-based emit position.
        let target = if hunk.old_len == 0 {
            // insert after old_start lines
            hunk.old_start
        } else {
            hunk.old_start
                .checked_sub(1)
                .ok_or(DiffError::BadHunkHeader)?
        };
        if target > a.len() || target < cursor {
            return Err(DiffError::ContextMismatch);
        }
        // emit unchanged lines between cursor and the hunk start
        for line in &a[cursor..target] {
            out.push_str(line);
        }
        cursor = target;

        for (tag, content) in &hunk.lines {
            match tag {
                Tag::Context | Tag::Removed => {
                    // must match old[cursor]
                    if cursor >= a.len() {
                        return Err(DiffError::ContextMismatch);
                    }
                    if !lines_match(a[cursor], content) {
                        return Err(DiffError::ContextMismatch);
                    }
                    if matches!(tag, Tag::Context) {
                        out.push_str(a[cursor]);
                    }
                    cursor += 1;
                }
                Tag::Added => {
                    // content is stored verbatim (its own newline preserved, or
                    // stripped by the no-newline marker), so emit it as-is.
                    out.push_str(content);
                }
            }
        }
    }
    // emit the remainder of old.
    if cursor > a.len() {
        return Err(DiffError::ContextMismatch);
    }
    for line in &a[cursor..] {
        out.push_str(line);
    }
    Ok(out)
}

/// Compare a stored `old` line (which keeps its trailing `\n`) against a patch
/// body `content` (which may or may not carry a `\n` depending on the source),
/// treating a trailing newline as insignificant for the match.
fn lines_match(old_line: &str, content: &str) -> bool {
    old_line.trim_end_matches('\n') == content.trim_end_matches('\n')
}

// ===========================================================================
// 3. Byte/binary delta — the delta-update primitive.
// ===========================================================================

/// One operation in a binary delta produced by [`byte_delta`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaOp {
    /// Copy `len` bytes starting at `src_off` in the OLD buffer.
    Copy { src_off: usize, len: usize },
    /// Insert these literal bytes (present in NEW, absent at this position in OLD).
    Data(Vec<u8>),
}

/// Compute a binary delta turning `old` into `new`.
///
/// **First-cut algorithm (documented):** common-prefix + common-suffix
/// detection with a single literal middle. We emit `Copy` for the shared prefix,
/// `Data` for the differing middle of `new`, and `Copy` for the shared suffix.
/// This is exact and cheap and already wins big for the OS-update case where a
/// new system image differs from the old in a bounded region. Rolling-hash block
/// matching (xdelta/bsdiff-style, matching repeated interior blocks) is a future
/// bonus; the [`DeltaOp`] format here already supports it (multiple `Copy`s at
/// arbitrary offsets) so upgrading the encoder does not change the wire format.
///
/// [`apply_delta`] reconstructs `new` exactly. Never panics.
pub fn byte_delta(old: &[u8], new: &[u8]) -> Vec<DeltaOp> {
    let mut ops = Vec::new();

    // common prefix length
    let max_pre = old.len().min(new.len());
    let mut prefix = 0usize;
    while prefix < max_pre && old[prefix] == new[prefix] {
        prefix += 1;
    }

    // common suffix length, not overlapping the prefix in either buffer
    let mut suffix = 0usize;
    let old_rem = old.len() - prefix;
    let new_rem = new.len() - prefix;
    let max_suf = old_rem.min(new_rem);
    while suffix < max_suf && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix] {
        suffix += 1;
    }

    if prefix > 0 {
        ops.push(DeltaOp::Copy {
            src_off: 0,
            len: prefix,
        });
    }
    // literal middle of NEW
    let mid_start = prefix;
    let mid_end = new.len() - suffix;
    if mid_end > mid_start {
        ops.push(DeltaOp::Data(new[mid_start..mid_end].to_vec()));
    }
    if suffix > 0 {
        ops.push(DeltaOp::Copy {
            src_off: old.len() - suffix,
            len: suffix,
        });
    }
    ops
}

/// Reconstruct `new` from `old` and a delta. Returns `Err(DeltaOutOfRange)` if a
/// `Copy` references bytes outside `old` (a corrupt/forged delta) — never panics.
pub fn apply_delta(old: &[u8], delta: &[DeltaOp]) -> Result<Vec<u8>, DiffError> {
    let mut out = Vec::new();
    for op in delta {
        match op {
            DeltaOp::Copy { src_off, len } => {
                let end = src_off
                    .checked_add(*len)
                    .ok_or(DiffError::DeltaOutOfRange)?;
                if end > old.len() {
                    return Err(DiffError::DeltaOutOfRange);
                }
                out.extend_from_slice(&old[*src_off..end]);
            }
            DeltaOp::Data(bytes) => out.extend_from_slice(bytes),
        }
    }
    Ok(out)
}

// ===========================================================================
// Host KATs — the FAIL-able proof (`cargo test -p rae_diff`).
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn rt_lines(old: &str, new: &str) {
        let ops = diff_lines(old, new);
        let got = apply_line_ops(old, &ops).expect("apply_line_ops");
        assert_eq!(got, new, "line-diff round-trip failed");
    }

    #[test]
    fn line_identical_all_equal() {
        let s = "a\nb\nc\n";
        let ops = diff_lines(s, s);
        // identical inputs => exactly one Equal op covering all lines, no edits.
        assert_eq!(ops, vec![DiffOp::Equal { start: 0, len: 3 }]);
        rt_lines(s, s);
    }

    #[test]
    fn line_insert_only() {
        let old = "a\nb\n";
        let new = "a\nX\nb\n";
        rt_lines(old, new);
        let ops = diff_lines(old, new);
        // must contain an Insert of "X\n"
        assert!(ops
            .iter()
            .any(|o| matches!(o, DiffOp::Insert { lines } if lines == &vec!["X\n".to_string()])));
        // and no Delete (pure insert)
        assert!(!ops.iter().any(|o| matches!(o, DiffOp::Delete { .. })));
    }

    #[test]
    fn line_delete_only() {
        let old = "a\nb\nc\n";
        let new = "a\nc\n";
        rt_lines(old, new);
        let ops = diff_lines(old, new);
        assert!(ops.iter().any(|o| matches!(o, DiffOp::Delete { .. })));
        assert!(!ops.iter().any(|o| matches!(o, DiffOp::Insert { .. })));
    }

    #[test]
    fn line_full_replace() {
        let old = "a\nb\nc\n";
        let new = "x\ny\nz\n";
        rt_lines(old, new);
    }

    #[test]
    fn line_known_script() {
        // A known change: middle line replaced.
        let old = "1\n2\n3\n";
        let new = "1\nTWO\n3\n";
        let ops = diff_lines(old, new);
        // Expect: Equal(0,1), then a Delete of line 1 and Insert "TWO\n", then Equal(2,1).
        // (order of Delete/Insert may vary; verify by round-trip + presence)
        rt_lines(old, new);
        assert!(ops
            .iter()
            .any(|o| matches!(o, DiffOp::Insert { lines } if lines == &vec!["TWO\n".to_string()])));
        assert!(ops
            .iter()
            .any(|o| matches!(o, DiffOp::Delete { start, len } if *start == 1 && *len == 1)));
        assert_eq!(ops.first(), Some(&DiffOp::Equal { start: 0, len: 1 }));
    }

    #[test]
    fn line_empty_inputs() {
        rt_lines("", "");
        rt_lines("", "a\nb\n");
        rt_lines("a\nb\n", "");
    }

    #[test]
    fn line_no_trailing_newline() {
        rt_lines("a\nb", "a\nc");
        rt_lines("hello", "hello world");
    }

    // ---- unified diff round-trip ----

    fn rt_unified(old: &str, new: &str, ctx: usize) {
        let u = unified_diff(old, new, ctx);
        let patch = parse_unified(&u).expect("parse_unified");
        let got = apply(old, &patch).expect("apply");
        assert_eq!(
            got, new,
            "unified round-trip failed for ctx={ctx}\n--- unified ---\n{u}"
        );
    }

    #[test]
    fn unified_header_math_known() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nX\nd\ne\n";
        let u = unified_diff(old, new, 1);
        // change at line 3; ctx=1 => hunk header @@ -2,3 +2,3 @@
        assert!(u.contains("@@ -2,3 +2,3 @@"), "got:\n{u}");
        assert!(u.contains("-c\n"), "got:\n{u}");
        assert!(u.contains("+X\n"), "got:\n{u}");
        assert!(u.contains(" b\n"));
        assert!(u.contains(" d\n"));
    }

    #[test]
    fn unified_roundtrip_battery() {
        rt_unified("a\nb\nc\n", "a\nb\nc\n", 3); // identical
        rt_unified("a\nb\nc\nd\ne\n", "a\nb\nX\nd\ne\n", 1);
        rt_unified("a\nb\nc\n", "x\ny\nz\n", 3); // full replace
        rt_unified("a\nb\n", "a\nINS\nb\n", 3); // insert
        rt_unified("a\nb\nc\n", "a\nc\n", 3); // delete
        rt_unified("", "a\nb\n", 3); // from empty
        rt_unified("a\nb\n", "", 3); // to empty
        rt_unified(
            "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n",
            "l1\nl2\nX\nl4\nl5\nl6\nY\nl8\n",
            2,
        ); // two hunks
        rt_unified("a\nb", "a\nc", 3); // no trailing newline
    }

    #[test]
    fn unified_identical_empty() {
        assert_eq!(unified_diff("a\nb\n", "a\nb\n", 3), "");
    }

    #[test]
    fn apply_mismatch_is_err_not_panic() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nX\nd\ne\n";
        let u = unified_diff(old, new, 1);
        let patch = parse_unified(&u).unwrap();
        // Apply against a DIFFERENT old whose context doesn't match.
        let wrong = "a\nb\nZZZ\nd\ne\n";
        match apply(wrong, &patch) {
            Err(DiffError::ContextMismatch) => {}
            other => panic!("expected ContextMismatch, got {other:?}"),
        }
    }

    // ---- malformed-input battery: must Err, never panic ----

    #[test]
    fn malformed_headers_err() {
        let cases = [
            "@@ garbage @@\n a\n",
            "@@ -x,1 +1,1 @@\n a\n",
            "@@ -1,1 1,1 @@\n a\n",
            "@@ -1,1 +1,1\n a\n",      // missing closing @@
            "@@ -1, +1,1 @@\n a\n",    // empty len
            "@@ -1,1 +1,1 @@\n?bad\n", // bad body marker
            "@@ -1,2 +1,1 @@\n a\n",   // truncated (declared 2 old, 1 given)
            "@@ -1,1 +1,1 @@",         // header only, truncated body
        ];
        for c in &cases {
            let r = parse_unified(c);
            assert!(r.is_err(), "expected Err for malformed: {c:?}");
        }
        // pure garbage with no @@ => empty patch, applies as identity (no panic).
        let r = parse_unified("just some random text\nno hunks here\n");
        assert_eq!(r, Ok(Patch { hunks: vec![] }));
        assert_eq!(apply("hello\n", &r.unwrap()).unwrap(), "hello\n");
    }

    // ---- multibyte / char-boundary safety (regression: lib.rs:553) ----

    #[test]
    fn unified_multibyte_content_roundtrips() {
        // Content AFTER the ASCII marker is multibyte. Generated diffs carry
        // these as " café\n" / "+résumé\n" etc.; parse_unified slices off the
        // 1-byte marker — must stay on a char boundary and round-trip.
        rt_unified("café\nplain\n", "café\nrésumé\n", 3);
        rt_unified("é\nb\nc\n", "é\nX\nc\n", 1);
        rt_unified("naïve\n", "naïve façade\n", 3);
        rt_unified("日本語\nb\n", "日本語\nX\n", 3); // multi-byte, multi-codepoint
    }

    #[test]
    fn parse_body_line_starting_multibyte_is_err_not_panic() {
        // The previously-panicking path: a hunk-body line whose FIRST byte is a
        // multibyte lead byte (no ASCII marker). The old `&line[1..]` sliced at
        // byte 1 — mid-codepoint — and panicked. It must now return BadHunkLine.
        // Build the raw diff by hand so the body line truly begins with `é`.
        let raw = "@@ -1,1 +1,1 @@\nécontext\n";
        // Sanity: this body line's first char IS multibyte, so byte 1 is NOT a
        // char boundary — i.e. this test actually exercises the fixed path.
        let body = raw.lines().nth(1).unwrap();
        assert!(
            !body.is_char_boundary(1),
            "test no longer exercises the mid-codepoint slice path"
        );
        match parse_unified(raw) {
            Err(DiffError::BadHunkLine) => {}
            other => panic!("expected BadHunkLine, got {other:?}"),
        }

        // A multibyte CONTEXT line (ASCII marker + multibyte content) followed
        // by the `\ No newline` marker parses and applies without panicking.
        let raw2 = "@@ -1,1 +1,1 @@\n café\n\\ No newline at end of file\n";
        let p = parse_unified(raw2).expect("parse_unified");
        assert_eq!(apply("café\n", &p).expect("apply"), "café\n");
    }

    // ---- byte delta round-trip ----

    fn rt_delta(old: &[u8], new: &[u8]) {
        let d = byte_delta(old, new);
        let got = apply_delta(old, &d).expect("apply_delta");
        assert_eq!(got, new, "byte-delta round-trip failed");
    }

    #[test]
    fn delta_battery() {
        rt_delta(b"", b"");
        rt_delta(b"hello", b"hello"); // identical
        rt_delta(b"hello", b"hello world"); // append
        rt_delta(b"world", b"hello world"); // prepend
        rt_delta(b"the quick brown fox", b"the slow brown fox"); // middle edit
        rt_delta(b"", b"brand new"); // from empty
        rt_delta(b"to be deleted", b""); // to empty
        rt_delta(b"abcdef", b"abXYef"); // interior replace, shared prefix+suffix
        rt_delta(&[0u8; 1000], &{
            let mut v = vec![0u8; 1000];
            v[500] = 1;
            v
        }); // large, single-byte interior change => small delta
    }

    #[test]
    fn delta_shape_is_small_for_interior_change() {
        let old = vec![7u8; 4096];
        let mut new = old.clone();
        new[2048] = 42;
        let d = byte_delta(&old, &new);
        // prefix copy + 1-byte data + suffix copy = 3 ops, tiny literal.
        let literal: usize = d
            .iter()
            .map(|o| match o {
                DeltaOp::Data(b) => b.len(),
                _ => 0,
            })
            .sum();
        assert!(
            literal <= 1,
            "interior change should ship ~1 literal byte, got {literal}"
        );
    }

    #[test]
    fn delta_corrupt_copy_is_err() {
        let bad = vec![DeltaOp::Copy {
            src_off: 10,
            len: 5,
        }];
        match apply_delta(b"abc", &bad) {
            Err(DiffError::DeltaOutOfRange) => {}
            other => panic!("expected DeltaOutOfRange, got {other:?}"),
        }
        // overflow case
        let bad2 = vec![DeltaOp::Copy {
            src_off: usize::MAX,
            len: 1,
        }];
        assert!(apply_delta(b"abc", &bad2).is_err());
    }
}
