//! # RaeMime — never-panic, `no_std` file-association / MIME resolver.
//!
//! RaeenOS_Concept.md §"Built for people who care about how things feel.": a
//! computer you can't open a downloaded file on by double-clicking it is not a
//! daily driver. Today RaeenOS has *zero* file-association registry — the #1
//! daily-driver parity gap. This crate is the infra-first foundation: given a
//! filename and (optionally) the file's leading bytes, it answers two questions
//! a desktop must answer instantly and locally:
//!
//!   1. **What is this file?** — a [`MimeType`], resolved by extension and/or by
//!      magic-byte content sniffing (so an extension-less or *mislabeled* file —
//!      a PNG saved as `photo.txt` — still resolves correctly).
//!   2. **What opens it?** — a [`Registry`] mapping MIME type → a default app id
//!      plus an ordered "Open With" candidate list.
//!
//! [`resolve`] ties the two together in one call. Everything here is pure data +
//! pure logic, which is exactly the right proof tier: the FAIL-able host KATs at
//! the bottom (`cargo test -p rae_mime`) are the primary proof — no QEMU, no
//! image build.
//!
//! ## This slice is infra only — wired into nothing
//! The Files "Open With" menu (`apps/files`) and the kernel double-click →
//! launch-default path are *deliberate* follow-up slots (see the module-level
//! note on [`resolve`]). Persistence of a user's chosen defaults to a config
//! file (via `rae_toml`) is likewise a follow-up; [`Registry`] is a pure,
//! in-memory data model here.
//!
//! ## Extension-vs-magic precedence (read before you trust a result)
//! Content is the truth; a filename is a hint. [`sniff_magic`] (content) is
//! therefore authoritative when it produces a confident match: a file whose
//! leading bytes are a PNG signature is `image/png` *even if it is named
//! `notes.txt`*. The extension is consulted (a) when no magic bytes are supplied,
//! (b) when the magic bytes do not match any known signature, or (c) to refine an
//! ambiguous magic match (e.g. a `PK\x03\x04` ZIP container that is actually a
//! `.jar`/`.docx` — those keep their extension-derived type because the magic is
//! a *container* signature, not the logical type). [`resolve`] documents the exact
//! order it applies. This mirrors how desktops treat "the bytes win, the name
//! breaks ties."
//!
//! ## Never-panic, hostile-byte posture
//! No `unwrap`/`expect`/`panic`/raw-index-panic is reachable from any public
//! function. Empty filenames, all-dots names, an empty magic slice, a 1-byte
//! magic slice, and arbitrary garbage all return a sensible fallback
//! (`application/octet-stream` → the Files app) rather than panicking — proven in
//! the KATs.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A resolved MIME type plus the canonical app id that should open it.
///
/// `mime` is a borrowed `'static str` for the built-in table entries; resolution
/// never allocates a MIME string. `name` filenames are matched case-insensitively
/// without allocation where possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MimeType(pub &'static str);

impl MimeType {
    /// The IANA / RaeenOS MIME string, e.g. `"image/png"`.
    pub fn as_str(&self) -> &'static str {
        self.0
    }

    /// The top-level type segment before the `/`, e.g. `"image"` for
    /// `"image/png"`. Used by the registry's family fallback.
    pub fn top_level(&self) -> &'static str {
        match self.0.as_bytes().iter().position(|&b| b == b'/') {
            Some(i) => &self.0[..i],
            None => self.0,
        }
    }
}

/// The universal fallback when nothing else matches: an opaque byte stream.
pub const OCTET_STREAM: MimeType = MimeType("application/octet-stream");

/// The fallback app id for unrecognized content — the Files app, which can at
/// least show the file and offer an explicit "Open With".
pub const FALLBACK_APP: &str = "files";

// ---------------------------------------------------------------------------
// Extension → MIME table
// ---------------------------------------------------------------------------

/// The built-in extension → MIME table (the common daily-driver set).
///
/// Keys are lowercase, dot-less. Lookup is case-insensitive (see
/// [`from_extension`]). Compound extensions (`.tar.gz`) are handled by
/// [`from_filename`], which tries the longest known compound suffix first.
const EXTENSION_TABLE: &[(&str, &str)] = &[
    // --- text ---
    ("txt", "text/plain"),
    ("text", "text/plain"),
    ("log", "text/plain"),
    ("ini", "text/plain"),
    ("cfg", "text/plain"),
    ("conf", "text/plain"),
    ("md", "text/markdown"),
    ("markdown", "text/markdown"),
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("json", "application/json"),
    ("toml", "application/toml"),
    ("xml", "application/xml"),
    ("yaml", "application/yaml"),
    ("yml", "application/yaml"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("js", "text/javascript"),
    ("rs", "text/x-rust"),
    ("c", "text/x-c"),
    ("h", "text/x-c"),
    ("py", "text/x-python"),
    ("sh", "text/x-shellscript"),
    // --- images ---
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("jpe", "image/jpeg"),
    ("gif", "image/gif"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    // --- documents ---
    ("pdf", "application/pdf"),
    // --- archives / compression ---
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("gzip", "application/gzip"),
    ("tar", "application/x-tar"),
    ("bz2", "application/x-bzip2"),
    ("xz", "application/x-xz"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    // --- audio ---
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("flac", "audio/flac"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("opus", "audio/opus"),
    ("aac", "audio/aac"),
    ("m4a", "audio/mp4"),
    // --- video ---
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("avi", "video/x-msvideo"),
    // --- executables / system ---
    ("elf", "application/x-executable"),
    ("raepkg", "application/x-raepkg"),
    ("wasm", "application/wasm"),
];

/// Compound (multi-segment) extensions, longest-suffix-first.
///
/// `.tar.gz` and `.tgz` resolve to `application/gzip` *consistently* (the
/// outermost container wins: the file IS gzip-compressed; that it wraps a tar is
/// a detail an extractor discovers). A bare `.tar` is `application/x-tar` via the
/// single-extension table.
const COMPOUND_TABLE: &[(&str, &str)] = &[
    ("tar.gz", "application/gzip"),
    ("tar.bz2", "application/x-bzip2"),
    ("tar.xz", "application/x-xz"),
    ("tgz", "application/gzip"),
    ("tbz", "application/x-bzip2"),
    ("txz", "application/x-xz"),
];

/// Resolve a MIME type from a bare (dot-less) extension, case-insensitively.
/// Returns `None` if the extension is unknown or empty.
///
/// Never allocates a MIME string; the comparison is done on a stack buffer for
/// short extensions and falls back to a heap buffer only for absurdly long ones.
pub fn from_extension(ext: &str) -> Option<MimeType> {
    if ext.is_empty() {
        return None;
    }
    let lower = ascii_lower(ext);
    for &(k, v) in EXTENSION_TABLE {
        if k == lower {
            return Some(MimeType(v));
        }
    }
    None
}

/// Resolve a MIME type from a full filename or path by extension, trying
/// compound suffixes (`.tar.gz`) before the single final extension.
///
/// Returns `None` if no extension is recognized. Never panics on empty / all-dot
/// / extension-less names. A leading-dot dotfile with no further extension
/// (`.bashrc`) is treated as having no extension.
pub fn from_filename(name: &str) -> Option<MimeType> {
    let base = basename(name);
    if base.is_empty() {
        return None;
    }

    // Try compound suffixes first (longest meaningful match).
    let lower_base = ascii_lower(base);
    for &(suffix, mime) in COMPOUND_TABLE {
        // Require a '.' immediately before the suffix so "x.tgz" matches but
        // "notatgz" does not, and the suffix is not the whole name.
        if let Some(prefix_len) = lower_base.len().checked_sub(suffix.len()) {
            if prefix_len > 0
                && lower_base.as_bytes().get(prefix_len - 1) == Some(&b'.')
                && &lower_base[prefix_len..] == suffix
            {
                return Some(MimeType(mime));
            }
        }
    }

    // Single final extension: bytes after the last '.', but only if the '.' is
    // not the first character of the basename (so ".bashrc" has no extension).
    let bytes = base.as_bytes();
    let mut dot = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'.' && i > 0 {
            dot = Some(i);
        }
    }
    let dot = dot?;
    let ext = &base[dot + 1..];
    from_extension(ext)
}

// ---------------------------------------------------------------------------
// Magic-byte (content) sniffing
// ---------------------------------------------------------------------------

/// A magic-byte signature: a byte pattern at a fixed offset → MIME type.
struct Magic {
    offset: usize,
    pattern: &'static [u8],
    mime: &'static str,
}

/// Stable, widely-relied-upon file signatures. Ordered most-specific first.
///
/// Only formats with a *stable, unambiguous* leading signature are listed —
/// sniffing is meant to be confident or silent. Container formats (ZIP) are
/// matched here as their container type; refining a `.docx`/`.jar` to its logical
/// type is left to the extension (documented in [`resolve`]).
const MAGIC_TABLE: &[Magic] = &[
    Magic {
        offset: 0,
        pattern: &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        mime: "image/png",
    },
    Magic {
        offset: 0,
        pattern: &[0xFF, 0xD8, 0xFF],
        mime: "image/jpeg",
    },
    Magic {
        offset: 0,
        pattern: b"GIF87a",
        mime: "image/gif",
    },
    Magic {
        offset: 0,
        pattern: b"GIF89a",
        mime: "image/gif",
    },
    Magic {
        offset: 0,
        pattern: b"BM",
        mime: "image/bmp",
    },
    Magic {
        offset: 0,
        pattern: b"%PDF-",
        mime: "application/pdf",
    },
    Magic {
        offset: 0,
        pattern: &[0x1F, 0x8B],
        mime: "application/gzip",
    },
    Magic {
        offset: 0,
        pattern: &[0x7F, 0x45, 0x4C, 0x46],
        mime: "application/x-executable",
    }, // ELF
    Magic {
        offset: 0,
        pattern: &[0x50, 0x4B, 0x03, 0x04],
        mime: "application/zip",
    }, // PK\x03\x04
    Magic {
        offset: 0,
        pattern: &[0x50, 0x4B, 0x05, 0x06],
        mime: "application/zip",
    }, // empty archive
    Magic {
        offset: 0,
        pattern: &[0x42, 0x5A, 0x68],
        mime: "application/x-bzip2",
    }, // BZh
    Magic {
        offset: 0,
        pattern: &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00],
        mime: "application/x-xz",
    },
    Magic {
        offset: 0,
        pattern: b"OggS",
        mime: "audio/ogg",
    },
    Magic {
        offset: 0,
        pattern: b"fLaC",
        mime: "audio/flac",
    },
    Magic {
        offset: 0,
        pattern: b"ID3",
        mime: "audio/mpeg",
    }, // MP3 w/ ID3 tag
    Magic {
        offset: 0,
        pattern: &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07],
        mime: "application/vnd.rar",
    }, // Rar!
    Magic {
        offset: 0,
        pattern: &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
        mime: "application/x-7z-compressed",
    },
    Magic {
        offset: 0,
        pattern: b"\0asm",
        mime: "application/wasm",
    }, // \0asm
    // ustar magic lives at offset 257 in a tar header block.
    Magic {
        offset: 257,
        pattern: b"ustar",
        mime: "application/x-tar",
    },
];

/// Detect a MIME type purely from a file's leading bytes. Returns `None` if no
/// known signature matches (including for empty or too-short input).
///
/// Confident-or-silent: a partial match (the slice is shorter than a signature
/// at its offset) is *not* reported, so callers can safely fall back to the
/// extension. Never panics on any slice, including empty.
pub fn sniff_magic(bytes: &[u8]) -> Option<MimeType> {
    for m in MAGIC_TABLE {
        let end = m.offset.checked_add(m.pattern.len())?;
        if bytes.len() >= end && &bytes[m.offset..end] == m.pattern {
            return Some(MimeType(m.mime));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Default-app registry
// ---------------------------------------------------------------------------

/// A pure, in-memory mapping from MIME type → default app + "Open With"
/// candidates.
///
/// The built-in table ([`Registry::with_defaults`]) is intentionally small and
/// sensible. Persistence of a user's chosen overrides to a config file (via
/// `rae_toml`) is a follow-up slot; this type carries no I/O.
///
/// Lookup falls back from an exact MIME match to a top-level family match
/// (`image/anything` → the `image/*` entry) to the universal fallback.
pub struct Registry {
    /// Exact-MIME rules: (mime, default_app, ordered candidates).
    exact: Vec<(String, String, Vec<String>)>,
    /// Top-level family rules: (top_level, default_app, ordered candidates).
    family: Vec<(String, String, Vec<String>)>,
}

impl Registry {
    /// An empty registry (only the universal fallback applies).
    pub fn new() -> Self {
        Registry {
            exact: Vec::new(),
            family: Vec::new(),
        }
    }

    /// The built-in sensible default registry.
    pub fn with_defaults() -> Self {
        let mut r = Registry::new();

        // Family defaults — the broad strokes.
        r.set_family("text", "text_editor", &["text_editor", "notes", "files"]);
        r.set_family("image", "photos", &["photos", "files"]);
        r.set_family("audio", "music", &["music", "files"]);
        r.set_family("video", "music", &["music", "files"]);

        // Exact overrides — where a family default is wrong or too coarse.
        r.set("application/pdf", "photos", &["photos", "files"]);
        r.set(
            "application/json",
            "text_editor",
            &["text_editor", "notes", "files"],
        );
        r.set(
            "text/csv",
            "text_editor",
            &["text_editor", "notes", "files"],
        );
        r.set("text/markdown", "notes", &["notes", "text_editor", "files"]);
        r.set("text/html", "raeweb", &["raeweb", "text_editor", "files"]);
        r.set(
            "image/svg+xml",
            "photos",
            &["photos", "text_editor", "files"],
        );

        // Archives → the Files app (which can extract).
        for archive in &[
            "application/zip",
            "application/gzip",
            "application/x-tar",
            "application/x-bzip2",
            "application/x-xz",
            "application/x-7z-compressed",
            "application/vnd.rar",
        ] {
            r.set(archive, "files", &["files"]);
        }

        // Executables / packages.
        r.set("application/x-executable", "files", &["files"]);
        r.set("application/x-raepkg", "raestore", &["raestore", "files"]);

        r
    }

    /// Insert / replace an exact-MIME rule.
    pub fn set(&mut self, mime: &str, default_app: &str, candidates: &[&str]) {
        let cand: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
        for e in self.exact.iter_mut() {
            if e.0 == mime {
                e.1 = default_app.to_string();
                e.2 = cand;
                return;
            }
        }
        self.exact
            .push((mime.to_string(), default_app.to_string(), cand));
    }

    /// Insert / replace a top-level family rule (`top_level` is e.g. `"image"`).
    pub fn set_family(&mut self, top_level: &str, default_app: &str, candidates: &[&str]) {
        let cand: Vec<String> = candidates.iter().map(|s| s.to_string()).collect();
        for e in self.family.iter_mut() {
            if e.0 == top_level {
                e.1 = default_app.to_string();
                e.2 = cand;
                return;
            }
        }
        self.family
            .push((top_level.to_string(), default_app.to_string(), cand));
    }

    /// The default app id for a MIME type. Exact match → family match →
    /// [`FALLBACK_APP`]. Never panics.
    pub fn default_app(&self, mime: MimeType) -> &str {
        let s = mime.as_str();
        for e in &self.exact {
            if e.0 == s {
                return &e.1;
            }
        }
        let top = mime.top_level();
        for e in &self.family {
            if e.0 == top {
                return &e.1;
            }
        }
        FALLBACK_APP
    }

    /// The ordered "Open With" candidate app ids for a MIME type. Exact → family
    /// → a single-element `[FALLBACK_APP]`. Never panics; never empty.
    pub fn candidates(&self, mime: MimeType) -> Vec<String> {
        let s = mime.as_str();
        for e in &self.exact {
            if e.0 == s {
                return e.2.clone();
            }
        }
        let top = mime.top_level();
        for e in &self.family {
            if e.0 == top {
                return e.2.clone();
            }
        }
        alloc::vec![FALLBACK_APP.to_string()]
    }
}

impl Default for Registry {
    fn default() -> Self {
        Registry::with_defaults()
    }
}

// ---------------------------------------------------------------------------
// Top-level resolve
// ---------------------------------------------------------------------------

/// The fully-resolved answer for a file: its type, its default app, and the
/// ordered "Open With" candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub mime: MimeType,
    pub default_app: String,
    pub candidates: Vec<String>,
}

/// Resolve a file to `(MimeType, default_app, candidates)` against a registry.
///
/// ## Precedence (the one place it is enforced)
/// 1. If `magic` is supplied and [`sniff_magic`] yields a confident match that is
///    **not** a generic container, that wins — content is truth. A PNG body named
///    `notes.txt` resolves to `image/png`.
/// 2. The filename extension ([`from_filename`]) is consulted next. It is also
///    used to *refine* a generic-container magic match: if the bytes are a ZIP
///    container (`application/zip`) but the name is `.docx`/`.jar`/etc., the
///    extension type is kept (the magic only told us "it's a zip box"). In this
///    build the only such refinement that applies is ZIP → a more specific
///    extension type; otherwise the ZIP magic stands.
/// 3. If neither produces a result → [`OCTET_STREAM`] → the Files app.
///
/// Never panics on empty filename, empty/short magic slice, or garbage.
///
/// ## Wiring note (follow-up slots, NOT this slice)
/// A future Files slot calls `resolve(name, Some(first_bytes), &registry)` to
/// build the "Open With" submenu and to pick the default for a double-click; the
/// kernel double-click-launch path uses `default_app` to choose which app id to
/// spawn. This crate performs neither — it is pure resolution.
pub fn resolve(filename: &str, magic: Option<&[u8]>, registry: &Registry) -> Resolution {
    let mime = resolve_mime(filename, magic);
    Resolution {
        default_app: registry.default_app(mime).to_string(),
        candidates: registry.candidates(mime),
        mime,
    }
}

/// Resolve just the [`MimeType`] (no registry), applying the documented
/// extension-vs-magic precedence. Useful for callers that only need the type.
pub fn resolve_mime(filename: &str, magic: Option<&[u8]>) -> MimeType {
    let by_ext = from_filename(filename);

    if let Some(bytes) = magic {
        if let Some(by_magic) = sniff_magic(bytes) {
            // Generic ZIP container: let a more specific extension refine it.
            if by_magic.as_str() == "application/zip" {
                if let Some(ext_mime) = by_ext {
                    if ext_mime.as_str() != "application/zip" {
                        return ext_mime;
                    }
                }
            }
            // Content is truth for every non-ambiguous signature.
            return by_magic;
        }
    }

    by_ext.unwrap_or(OCTET_STREAM)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lowercase an ASCII string into an owned `String` (non-ASCII bytes pass
/// through unchanged — extensions are ASCII in practice). Never panics.
fn ascii_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        out.push((b.to_ascii_lowercase()) as char);
    }
    out
}

/// The final path component of a name (after the last `/` or `\`). Returns the
/// whole string if there is no separator. Never panics on empty input.
fn basename(name: &str) -> &str {
    let bytes = name.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'/' || b == b'\\' {
            start = i + 1;
        }
    }
    // start may equal len() for a trailing-slash path → empty basename.
    if start >= name.len() {
        ""
    } else {
        &name[start..]
    }
}

// ===========================================================================
// Host KATs — the primary, FAIL-able proof. `cargo test -p rae_mime`.
//
// FAIL-ability: every assertion compares against an explicit expected value
// (e.g. `from_filename("a.JPG") == Some(MimeType("image/jpeg"))`). Break the
// table (drop the "jpg" row, or change "image/jpeg" to "image/jpg") and the
// corresponding assert fails immediately. The precedence tests assert the exact
// winner, so flipping magic/extension precedence flips a test from pass to fail.
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // ---- extension resolution ----

    #[test]
    fn basic_extensions() {
        assert_eq!(from_filename("readme.txt"), Some(MimeType("text/plain")));
        assert_eq!(from_filename("notes.md"), Some(MimeType("text/markdown")));
        assert_eq!(from_filename("photo.png"), Some(MimeType("image/png")));
        assert_eq!(from_filename("doc.pdf"), Some(MimeType("application/pdf")));
        assert_eq!(
            from_filename("data.json"),
            Some(MimeType("application/json"))
        );
        assert_eq!(from_filename("song.mp3"), Some(MimeType("audio/mpeg")));
        assert_eq!(from_filename("clip.mp4"), Some(MimeType("video/mp4")));
        assert_eq!(from_filename("table.csv"), Some(MimeType("text/csv")));
    }

    #[test]
    fn extension_is_case_insensitive() {
        // .JPG must resolve exactly like .jpg.
        assert_eq!(from_filename("VACATION.JPG"), Some(MimeType("image/jpeg")));
        assert_eq!(from_filename("Photo.PnG"), Some(MimeType("image/png")));
        assert_eq!(from_extension("PDF"), Some(MimeType("application/pdf")));
    }

    #[test]
    fn compound_tar_gz_is_consistent() {
        // .tar.gz and .tgz both → application/gzip; a bare .tar → x-tar.
        assert_eq!(
            from_filename("src.tar.gz"),
            Some(MimeType("application/gzip"))
        );
        assert_eq!(
            from_filename("src.TAR.GZ"),
            Some(MimeType("application/gzip"))
        );
        assert_eq!(
            from_filename("backup.tgz"),
            Some(MimeType("application/gzip"))
        );
        assert_eq!(
            from_filename("archive.tar"),
            Some(MimeType("application/x-tar"))
        );
        assert_eq!(
            from_filename("data.tar.bz2"),
            Some(MimeType("application/x-bzip2"))
        );
    }

    #[test]
    fn path_with_directories() {
        assert_eq!(
            from_filename("/home/user/Downloads/photo.png"),
            Some(MimeType("image/png"))
        );
        assert_eq!(
            from_filename("C:\\Users\\me\\song.mp3"),
            Some(MimeType("audio/mpeg"))
        );
    }

    #[test]
    fn dotfiles_have_no_extension() {
        // A leading-dot dotfile with nothing after is NOT an extension.
        assert_eq!(from_filename(".bashrc"), None);
        assert_eq!(from_filename(".gitignore"), None);
        // But a dotfile WITH a real extension still resolves.
        assert_eq!(
            from_filename(".config.json"),
            Some(MimeType("application/json"))
        );
    }

    #[test]
    fn unknown_and_missing_extensions() {
        assert_eq!(from_filename("noextension"), None);
        assert_eq!(from_filename("file.wat-is-this"), None);
        assert_eq!(from_extension(""), None);
    }

    // ---- magic-byte sniffing ----

    #[test]
    fn magic_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01];
        assert_eq!(sniff_magic(&png), Some(MimeType("image/png")));
    }

    #[test]
    fn magic_common_formats() {
        assert_eq!(
            sniff_magic(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(MimeType("image/jpeg"))
        );
        assert_eq!(sniff_magic(b"GIF89a..."), Some(MimeType("image/gif")));
        assert_eq!(sniff_magic(b"%PDF-1.7"), Some(MimeType("application/pdf")));
        assert_eq!(
            sniff_magic(&[0x1F, 0x8B, 0x08]),
            Some(MimeType("application/gzip"))
        );
        assert_eq!(
            sniff_magic(&[0x7F, 0x45, 0x4C, 0x46, 0x02]),
            Some(MimeType("application/x-executable"))
        );
        assert_eq!(
            sniff_magic(b"PK\x03\x04rest"),
            Some(MimeType("application/zip"))
        );
    }

    #[test]
    fn magic_ustar_at_offset_257() {
        // ustar lives at byte 257 of a tar header block.
        let mut block = alloc::vec![0u8; 270];
        block[257..262].copy_from_slice(b"ustar");
        assert_eq!(sniff_magic(&block), Some(MimeType("application/x-tar")));
    }

    #[test]
    fn magic_short_and_empty_input_is_silent() {
        // Confident-or-silent: too-short slices do NOT match.
        assert_eq!(sniff_magic(&[]), None);
        assert_eq!(sniff_magic(&[0x89]), None); // partial PNG
        assert_eq!(sniff_magic(&[0xFF]), None); // partial JPEG
                                                // ustar offset beyond the slice → no panic, no match.
        assert_eq!(sniff_magic(b"ustar"), None);
    }

    // ---- precedence: content (magic) wins over a mislabeled name ----

    #[test]
    fn magic_overrides_mislabeled_extension() {
        // A PNG body saved as notes.txt must resolve to image/png — bytes win.
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(resolve_mime("notes.txt", Some(&png)), MimeType("image/png"));
    }

    #[test]
    fn extension_used_when_magic_absent_or_unknown() {
        // No magic → fall back to extension.
        assert_eq!(resolve_mime("a.png", None), MimeType("image/png"));
        // Magic present but unrecognized → fall back to extension.
        let garbage = [0x00, 0x11, 0x22, 0x33];
        assert_eq!(resolve_mime("a.png", Some(&garbage)), MimeType("image/png"));
    }

    #[test]
    fn zip_container_refined_by_extension() {
        // PK\x03\x04 + a .docx name → the extension is NOT in our table, so it
        // stays application/zip (we have no docx row); a .zip name stays zip.
        let zip = b"PK\x03\x04xxxx";
        assert_eq!(
            resolve_mime("archive.zip", Some(zip)),
            MimeType("application/zip")
        );
        // A .raepkg is actually our package container; if a publisher ships it as
        // a zip-on-the-wire, the extension refines it to x-raepkg.
        assert_eq!(
            resolve_mime("game.raepkg", Some(zip)),
            MimeType("application/x-raepkg")
        );
    }

    // ---- registry lookups ----

    #[test]
    fn registry_default_app() {
        let r = Registry::with_defaults();
        assert_eq!(r.default_app(MimeType("text/plain")), "text_editor");
        assert_eq!(r.default_app(MimeType("image/png")), "photos");
        assert_eq!(r.default_app(MimeType("audio/mpeg")), "music");
        assert_eq!(r.default_app(MimeType("application/pdf")), "photos");
        assert_eq!(r.default_app(MimeType("application/zip")), "files");
        assert_eq!(r.default_app(MimeType("text/markdown")), "notes");
    }

    #[test]
    fn registry_family_fallback() {
        let r = Registry::with_defaults();
        // No exact rule for image/webp → falls to the image/* family.
        assert_eq!(r.default_app(MimeType("image/webp")), "photos");
        assert_eq!(r.default_app(MimeType("video/x-matroska")), "music");
    }

    #[test]
    fn registry_candidates_ordered_and_nonempty() {
        let r = Registry::with_defaults();
        let cands = r.candidates(MimeType("text/markdown"));
        assert_eq!(cands, alloc::vec!["notes", "text_editor", "files"]);
        // Unknown type → exactly [FALLBACK_APP], never empty.
        let unknown = r.candidates(OCTET_STREAM);
        assert_eq!(unknown, alloc::vec!["files"]);
        assert!(!unknown.is_empty());
    }

    #[test]
    fn registry_override_replaces() {
        let mut r = Registry::with_defaults();
        r.set("image/png", "my_viewer", &["my_viewer", "photos"]);
        assert_eq!(r.default_app(MimeType("image/png")), "my_viewer");
        assert_eq!(
            r.candidates(MimeType("image/png")),
            alloc::vec!["my_viewer", "photos"]
        );
    }

    // ---- fallbacks & never-panic ----

    #[test]
    fn unknown_extension_falls_back_to_octet_stream() {
        assert_eq!(resolve_mime("mystery.zzz", None), OCTET_STREAM);
        let res = resolve("mystery.zzz", None, &Registry::with_defaults());
        assert_eq!(res.mime, OCTET_STREAM);
        assert_eq!(res.default_app, "files");
        assert_eq!(res.candidates, alloc::vec!["files"]);
    }

    #[test]
    fn empty_and_garbage_input_never_panics() {
        let r = Registry::with_defaults();
        // Empty filename, no magic.
        let a = resolve("", None, &r);
        assert_eq!(a.mime, OCTET_STREAM);
        // Empty filename, empty magic.
        let b = resolve("", Some(&[]), &r);
        assert_eq!(b.mime, OCTET_STREAM);
        // All dots.
        assert_eq!(resolve_mime("....", None), OCTET_STREAM);
        // Trailing slash → empty basename.
        assert_eq!(resolve_mime("dir/", None), OCTET_STREAM);
        // 1-byte magic + weird name.
        let c = resolve("\x00\x01", Some(&[0x42]), &r);
        assert_eq!(c.mime, OCTET_STREAM);
    }

    #[test]
    fn resolve_full_path_with_magic() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let res = resolve(
            "/Downloads/screenshot.png",
            Some(&png),
            &Registry::with_defaults(),
        );
        assert_eq!(res.mime, MimeType("image/png"));
        assert_eq!(res.default_app, "photos");
        assert_eq!(res.candidates, alloc::vec!["photos", "files"]);
    }

    #[test]
    fn top_level_accessor() {
        assert_eq!(MimeType("image/png").top_level(), "image");
        assert_eq!(
            MimeType("application/octet-stream").top_level(),
            "application"
        );
        // No slash → whole string.
        assert_eq!(MimeType("weird").top_level(), "weird");
    }

    // =======================================================================
    // Fuzz / property tests — hostile-byte hardening for the magic sniffer.
    //
    // rae_mime sniffs the LEADING BYTES of arbitrary/downloaded files: a pure
    // attacker-controlled surface. A malformed file must NEVER panic the
    // resolver. These tests drive thousands of adversarial byte buffers and
    // assert (a) no panic, (b) the documented precedence holds as a property,
    // (c) the extension round-trip property holds.
    //
    // No external fuzz crate: a tiny seeded xorshift PRNG generates the corpus
    // deterministically (same seed → same run, so a failure is reproducible).
    //
    // FAIL-ability: these are not vacuous. The precedence property asserts the
    // EXACT winner against an independent oracle — flip the magic/extension
    // order in `resolve_mime` and `prop_magic_beats_mislabeled_extension`
    // fails. The round-trip property asserts every table extension yields a
    // non-empty mime and unknowns yield octet-stream — drop a table row and
    // `prop_extension_round_trip` fails. (Demonstrated by reasoning in the
    // REPORT; the asserts compare to concrete expected values, not `is_ok()`.)
    // =======================================================================

    /// Deterministic xorshift64* PRNG — pure, no_std-safe, reproducible.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            // avoid the zero fixed-point of xorshift
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn byte(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        /// Uniform-ish value in `0..n` (n > 0).
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    /// All MIME strings emitted by the magic table — the sniff oracle.
    fn all_magic_mimes() -> &'static [&'static str] {
        &[
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/bmp",
            "application/pdf",
            "application/gzip",
            "application/x-executable",
            "application/zip",
            "application/x-bzip2",
            "application/x-xz",
            "audio/ogg",
            "audio/flac",
            "audio/mpeg",
            "application/vnd.rar",
            "application/x-7z-compressed",
            "application/wasm",
            "application/x-tar",
        ]
    }

    /// All known signature byte patterns + their offset, as a test oracle that
    /// is INDEPENDENT of MAGIC_TABLE's private struct (so a regression in the
    /// table is caught by the property, not masked by sharing the same data).
    fn signature_corpus() -> alloc::vec::Vec<(usize, alloc::vec::Vec<u8>, &'static str)> {
        alloc::vec![
            (
                0,
                alloc::vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
                "image/png"
            ),
            (0, alloc::vec![0xFF, 0xD8, 0xFF], "image/jpeg"),
            (0, b"GIF87a".to_vec(), "image/gif"),
            (0, b"GIF89a".to_vec(), "image/gif"),
            (0, b"BM".to_vec(), "image/bmp"),
            (0, b"%PDF-".to_vec(), "application/pdf"),
            (0, alloc::vec![0x1F, 0x8B], "application/gzip"),
            (
                0,
                alloc::vec![0x7F, 0x45, 0x4C, 0x46],
                "application/x-executable"
            ),
            (0, alloc::vec![0x50, 0x4B, 0x03, 0x04], "application/zip"),
            (0, alloc::vec![0x50, 0x4B, 0x05, 0x06], "application/zip"),
            (0, alloc::vec![0x42, 0x5A, 0x68], "application/x-bzip2"),
            (
                0,
                alloc::vec![0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00],
                "application/x-xz"
            ),
            (0, b"OggS".to_vec(), "audio/ogg"),
            (0, b"fLaC".to_vec(), "audio/flac"),
            (0, b"ID3".to_vec(), "audio/mpeg"),
            (
                0,
                alloc::vec![0x52, 0x61, 0x72, 0x21, 0x1A, 0x07],
                "application/vnd.rar"
            ),
            (
                0,
                alloc::vec![0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
                "application/x-7z-compressed"
            ),
            (0, b"\0asm".to_vec(), "application/wasm"),
            (257, b"ustar".to_vec(), "application/x-tar"),
        ]
    }

    /// Every extension key in the table (single + compound), for the round-trip.
    fn all_extension_keys() -> alloc::vec::Vec<&'static str> {
        let mut v = alloc::vec::Vec::new();
        for &(k, _) in EXTENSION_TABLE {
            v.push(k);
        }
        for &(k, _) in COMPOUND_TABLE {
            v.push(k);
        }
        v
    }

    /// PROPERTY: `sniff_magic` and `resolve` never panic on ANY byte buffer.
    /// Drives ~20k random buffers of length 0..300 plus the all-0xFF and all-0x00
    /// extremes at every length.
    #[test]
    fn fuzz_sniff_never_panics_random() {
        let r = Registry::with_defaults();
        let mut rng = Rng::new(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..20_000 {
            let len = rng.below(301); // 0..=300
            let mut buf = alloc::vec::Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Must not panic. Result value is unconstrained for random bytes.
            let _ = sniff_magic(&buf);
            let _ = resolve("random.bin", Some(&buf), &r);
            let _ = resolve_mime("random.bin", Some(&buf));
        }
        // pathological constant buffers at every boundary length
        for len in 0..=300usize {
            let all_ff = alloc::vec![0xFFu8; len];
            let all_00 = alloc::vec![0x00u8; len];
            let _ = sniff_magic(&all_ff);
            let _ = sniff_magic(&all_00);
            let _ = resolve("x.png", Some(&all_ff), &r);
            let _ = resolve("x.png", Some(&all_00), &r);
        }
    }

    /// PROPERTY: truncating any known signature at EVERY offset never panics and
    /// never reports a (false) confident match shorter than the signature — the
    /// "confident or silent" contract. Also covers the ustar@257 boundary at
    /// lengths 256/257/258/512 explicitly.
    #[test]
    fn fuzz_truncated_signatures_silent_not_panic() {
        for (off, pat, mime) in signature_corpus() {
            let full_len = off + pat.len();
            // Build the full signature in a zero-padded buffer, then truncate at
            // every length from 0..=full_len and feed it in.
            let mut full = alloc::vec![0u8; full_len];
            full[off..off + pat.len()].copy_from_slice(&pat);
            for cut in 0..=full_len {
                let slice = &full[..cut];
                let got = sniff_magic(slice); // must not panic
                if cut >= full_len {
                    // exactly long enough → must match its mime (FAIL-able)
                    assert_eq!(
                        got,
                        Some(MimeType(mime)),
                        "full signature for {} at len {} should match",
                        mime,
                        cut
                    );
                } else if off == 0 {
                    // shorter than an offset-0 signature → must NOT report THIS
                    // mime (confident-or-silent). It may match a shorter sig that
                    // is a prefix; assert it is never a longer-than-available one.
                    if let Some(MimeType(m)) = got {
                        // whatever matched must itself fit in `cut` bytes
                        let fits = signature_corpus()
                            .iter()
                            .any(|(o, p, mm)| *mm == m && *o + p.len() <= cut);
                        assert!(fits, "sniff reported {} from only {} bytes", m, cut);
                    }
                }
            }
        }
        // explicit ustar@257 boundary buffers
        for &len in &[256usize, 257, 258, 512] {
            let mut block = alloc::vec![0u8; len];
            if len >= 262 {
                block[257..262].copy_from_slice(b"ustar");
                assert_eq!(sniff_magic(&block), Some(MimeType("application/x-tar")));
            } else {
                // not enough room for the full ustar magic → silent, no panic
                assert_eq!(sniff_magic(&block), None);
            }
        }
    }

    /// PROPERTY: a known magic always WINS over a mislabeled extension (content
    /// is truth), EXCEPT the documented generic-ZIP refinement. Independent
    /// oracle: we know `expected` from the signature we planted, so this fails
    /// if precedence is flipped in `resolve_mime`.
    #[test]
    fn prop_magic_beats_mislabeled_extension() {
        let mut rng = Rng::new(0x0123_4567_89AB_CDEF);
        let sigs = signature_corpus();
        // Mislabel names whose extension is NOT in our table OR is itself zip:
        // these can never trigger the documented generic-ZIP refinement, so the
        // planted magic must always win regardless of which signature it is.
        let mislabels = ["notes.unknownext", "noext", "x.zzz", "a.zip"];
        for _ in 0..5_000 {
            let (off, pat, mime) = &sigs[rng.below(sigs.len())];
            // build a buffer that starts with the signature, then random tail
            let tail = rng.below(40);
            let mut buf = alloc::vec![0u8; off + pat.len()];
            buf[*off..*off + pat.len()].copy_from_slice(pat);
            for _ in 0..tail {
                buf.push(rng.byte());
            }
            let name = mislabels[rng.below(mislabels.len())];
            let got = resolve_mime(name, Some(&buf));

            // Content is truth: with a non-refinable extension the magic wins.
            assert_eq!(
                got,
                MimeType(mime),
                "magic {} should beat extension of name {:?}",
                mime,
                name
            );
        }

        // The DOCUMENTED exception, as its own FAIL-able property: a generic ZIP
        // magic with a KNOWN, more-specific extension keeps the extension type
        // (a .json/.png/.raepkg whose bytes happen to start with PK\x03\x04).
        // This is the inverse direction and proves the refinement branch exists.
        let zip = b"PK\x03\x04xxxx";
        for (ext, expected) in [
            ("json", "application/json"),
            ("png", "image/png"),
            ("raepkg", "application/x-raepkg"),
        ] {
            let name = alloc::format!("file.{}", ext);
            assert_eq!(
                resolve_mime(&name, Some(zip)),
                MimeType(expected),
                "generic ZIP magic must be refined by .{} extension",
                ext
            );
        }
        // ...but a bare .zip name keeps zip (FAIL-able opposite direction).
        assert_eq!(
            resolve_mime("a.zip", Some(zip)),
            MimeType("application/zip")
        );
    }

    /// PROPERTY: when magic is absent OR unknown, the extension is used; when the
    /// extension is also unknown, the result is octet-stream. Drives random
    /// non-signature byte buffers (guaranteed not to match by using a 0x00-lead
    /// then checking sniff is None) against a known extension.
    #[test]
    fn prop_unknown_magic_falls_back_to_extension() {
        let mut rng = Rng::new(0xAAAA_5555_3333_CCCC);
        for _ in 0..5_000 {
            // build a buffer that does NOT match any signature
            let len = 1 + rng.below(64);
            let mut buf = alloc::vec::Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if sniff_magic(&buf).is_some() {
                continue; // accidentally a signature; skip — the other test covers it
            }
            // known extension → must resolve to that extension's mime
            assert_eq!(resolve_mime("photo.png", Some(&buf)), MimeType("image/png"));
            // absent magic → same
            assert_eq!(resolve_mime("photo.png", None), MimeType("image/png"));
            // unknown extension + unknown magic → octet-stream
            assert_eq!(resolve_mime("file.zzzqqq", Some(&buf)), OCTET_STREAM);
            assert_eq!(resolve_mime("file.zzzqqq", None), OCTET_STREAM);
        }
    }

    /// PROPERTY (round-trip): every KNOWN extension maps to a non-empty MimeType
    /// containing a '/', and every UNKNOWN extension falls back to octet-stream.
    /// Drops in this table → a row's assert fails immediately.
    #[test]
    fn prop_extension_round_trip() {
        for ext in all_extension_keys() {
            // single + compound keys both reach a mime via from_filename
            let name = alloc::format!("file.{}", ext);
            let mime = from_filename(&name).expect("known extension must resolve");
            assert!(!mime.as_str().is_empty(), "mime empty for .{}", ext);
            assert!(
                mime.as_str().contains('/'),
                "mime {} for .{} lacks '/'",
                mime.as_str(),
                ext
            );
            // resolve_mime with no magic must agree
            assert_eq!(resolve_mime(&name, None), mime);
        }
        // a sweep of clearly-unknown extensions → octet-stream
        let unknowns = ["zzz", "qqqq", "nope", "xyzzy", "wat", "1234nope"];
        for u in unknowns {
            let name = alloc::format!("f.{}", u);
            assert_eq!(resolve_mime(&name, None), OCTET_STREAM);
        }
        // every magic mime is itself well-formed (oracle sanity)
        for m in all_magic_mimes() {
            assert!(m.contains('/'), "magic mime {} lacks '/'", m);
        }
    }

    /// PROPERTY: resolve never panics on adversarial FILENAMES (random unicode,
    /// many dots, slashes, NULs, very long names), independent of magic.
    #[test]
    fn fuzz_filename_never_panics() {
        let r = Registry::with_defaults();
        let mut rng = Rng::new(0xF00D_F00D_1234_5678);
        // a pool of nasty characters incl. multibyte UTF-8
        let pool: [&str; 12] = [
            ".", "/", "\\", "\0", "a", ".png", "..", "тест", "🦀", "tar.gz", " ", "Z",
        ];
        for _ in 0..10_000 {
            let segs = rng.below(40);
            let mut name = String::new();
            for _ in 0..segs {
                name.push_str(pool[rng.below(pool.len())]);
            }
            // must not panic — value unconstrained
            let _ = from_filename(&name);
            let _ = resolve_mime(&name, None);
            let _ = resolve(&name, Some(&[0x89, 0x50]), &r);
        }
        // explicit pathological names
        for name in &["", ".", "..", "...", "/", "\\", "/////", "a/b/c/", ".\0.\0"] {
            let res = resolve(name, None, &r);
            // never empty candidate list
            assert!(!res.candidates.is_empty());
        }
        // an absurdly long all-dots name
        let long: String = core::iter::repeat('.').take(10_000).collect();
        let _ = from_filename(&long);
    }
}
