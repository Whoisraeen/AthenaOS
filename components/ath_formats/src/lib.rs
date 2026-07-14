//! # RaeFormats — a never-panic, `no_std` content-type sniffer.
//!
//! LEGACY_GAMING_CONCEPT.md criterion #5 (the cohesion seam — every part of the OS
//! agrees on what a file *is*) and criterion #6 (security never trusts a file
//! extension): a daily driver must detect a file's **true** type from its bytes so
//! that Files, Quick Look, and the decoder dispatch all pick the same correct
//! handler — and so the security layer never decides what to do with a file based
//! on a name an attacker controls (`invoice.pdf.exe`, a PE renamed `.png`).
//!
//! This crate answers two questions and *only* those two:
//!   1. **What IS this?** — [`detect`] / [`detect_with_hint`] return a [`FileKind`].
//!   2. **Which decoder handles it?** — [`FileKind::recommended_decoder`] returns
//!      the crate *name* (a `&str`, never a dependency).
//!
//! The caller then invokes that decoder. By design this crate **depends on no
//! decoder crate** (no `ath_png`/`ath_pdf`/`ath_mp4`/…): a sniffer that pulled in
//! every decoder would become the heaviest dependency hub in the tree. All
//! detection is bounded byte peeks into the leading / structural bytes.
//!
//! ## The whole value is the ambiguous families
//! Several real formats share a leading signature; getting these right is the
//! point of the crate (the host KATs at the bottom prove each disambiguation):
//!   - **ZIP container family** — OOXML (`.docx`/`.xlsx`/`.pptx`), ODF, JAR, EPUB,
//!     and a plain ZIP all begin `PK\x03\x04`. We peek the *first local-file
//!     entry's name* (bounded, without unzipping): `word/` → Docx, `xl/` → Xlsx,
//!     `ppt/` → Pptx, a `mimetype` entry → inspect its stored ODF/EPUB MIME,
//!     otherwise generic Zip.
//!   - **RIFF family** — WAV, AVI, and WebP all begin `RIFF....` then carry a
//!     4-byte *form type* at offset 8: `WAVE` → Wav, `AVI ` → Avi, `WEBP` → Webp.
//!   - **ISO-BMFF / MP4** — detected by the `ftyp` box at offset 4; the major
//!     brand (`isom`/`mp4*`/`avc1` → Mp4 video, `M4A `/`M4B ` → M4a audio, `qt  `
//!     → QuickTime, `heic`/`heif` → Heif).
//!   - **EBML** — Matroska and WebM both begin `1A 45 DF A3`; a bounded scan for a
//!     `webm` DocType string picks WebM, otherwise Matroska.
//!   - **MP3** — an `ID3` tag *or* a valid MPEG-audio frame-sync header.
//!
//! ## Never-panic, hostile-byte posture
//! Every input is attacker-controlled (downloads, email attachments, untrusted
//! shares). No `unwrap`/`expect`/`panic`/raw-index-panic is reachable: empty,
//! 1-byte, truncated, and adversarial buffers all return [`FileKind::Unknown`] (or
//! the correct kind) — proven by the seeded fuzz loop in the KATs.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

/// The detected type of a file. Covers the formats AthenaOS already handles plus
/// the common daily-driver set; [`FileKind::Unknown`] is the honest fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    // ---- images ----
    Png,
    Jpeg,
    Gif,
    Bmp,
    Webp,
    Tiff,
    Ico,
    /// SVG is XML text; detected heuristically (`<svg`/`<?xml`+`<svg`).
    Svg,
    /// HEIF / HEIC still images (ISO-BMFF with a `heic`/`heif`/`mif1` brand).
    Heif,
    // ---- documents ----
    Pdf,
    /// OOXML word processing (ZIP whose first entry is under `word/`).
    Docx,
    /// OOXML spreadsheet (ZIP whose first entry is under `xl/`).
    Xlsx,
    /// OOXML presentation (ZIP whose first entry is under `ppt/`).
    Pptx,
    /// OpenDocument text (ZIP with a `mimetype` of `…opendocument.text`).
    OdfText,
    /// OpenDocument spreadsheet.
    OdfSheet,
    /// OpenDocument presentation.
    OdfPres,
    /// EPUB e-book (ZIP with a `mimetype` of `application/epub+zip`).
    Epub,
    Rtf,
    PlainText,
    /// Markdown — heuristic over text (extension hint or structural markers).
    Markdown,
    /// HTML — heuristic (`<!doctype html`/`<html`).
    Html,
    // ---- media containers / streams ----
    /// MP4 / ISO-BMFF video (an `ftyp` box with a video brand).
    Mp4,
    /// MPEG-4 audio (`.m4a`/`.m4b`: an `ftyp` box with an audio brand).
    M4a,
    /// Apple QuickTime movie (`ftyp` brand `qt  ` or `moov` at offset 4).
    QuickTime,
    /// Matroska container.
    Matroska,
    /// WebM (Matroska profile with a `webm` DocType).
    WebM,
    /// Ogg container (Vorbis/Opus/Theora).
    Ogg,
    Wav,
    Flac,
    /// MP3 (ID3 tag or a raw MPEG-audio frame sync).
    Mp3,
    /// AAC in an ADTS stream.
    Aac,
    Avi,
    // ---- archives / compression ----
    Zip,
    Gzip,
    Tar,
    Xz,
    Zstd,
    Bzip2,
    SevenZip,
    Rar,
    // ---- executables / runtimes / fonts ----
    Elf,
    /// Windows PE / COFF executable or DLL (`MZ` header).
    Pe,
    Wasm,
    Ttf,
    Otf,
    Woff,
    Woff2,
    /// Type unknown / no confident match.
    Unknown,
}

/// A coarse grouping used by Files / the launcher to pick an icon and a default
/// "Open With" family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Image,
    Document,
    Audio,
    Video,
    Archive,
    Executable,
    Font,
    Text,
    Other,
}

impl FileKind {
    /// The canonical MIME type string for this kind. Never allocates.
    pub fn mime_type(self) -> &'static str {
        use FileKind::*;
        match self {
            Png => "image/png",
            Jpeg => "image/jpeg",
            Gif => "image/gif",
            Bmp => "image/bmp",
            Webp => "image/webp",
            Tiff => "image/tiff",
            Ico => "image/x-icon",
            Svg => "image/svg+xml",
            Heif => "image/heif",
            Pdf => "application/pdf",
            Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            Pptx => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            OdfText => "application/vnd.oasis.opendocument.text",
            OdfSheet => "application/vnd.oasis.opendocument.spreadsheet",
            OdfPres => "application/vnd.oasis.opendocument.presentation",
            Epub => "application/epub+zip",
            Rtf => "application/rtf",
            PlainText => "text/plain",
            Markdown => "text/markdown",
            Html => "text/html",
            Mp4 => "video/mp4",
            M4a => "audio/mp4",
            QuickTime => "video/quicktime",
            Matroska => "video/x-matroska",
            WebM => "video/webm",
            Ogg => "application/ogg",
            Wav => "audio/wav",
            Flac => "audio/flac",
            Mp3 => "audio/mpeg",
            Aac => "audio/aac",
            Avi => "video/x-msvideo",
            Zip => "application/zip",
            Gzip => "application/gzip",
            Tar => "application/x-tar",
            Xz => "application/x-xz",
            Zstd => "application/zstd",
            Bzip2 => "application/x-bzip2",
            SevenZip => "application/x-7z-compressed",
            Rar => "application/vnd.rar",
            Elf => "application/x-executable",
            Pe => "application/vnd.microsoft.portable-executable",
            Wasm => "application/wasm",
            Ttf => "font/ttf",
            Otf => "font/otf",
            Woff => "font/woff",
            Woff2 => "font/woff2",
            Unknown => "application/octet-stream",
        }
    }

    /// A canonical (dot-less) file extension for this kind. Never allocates.
    pub fn extension(self) -> &'static str {
        use FileKind::*;
        match self {
            Png => "png",
            Jpeg => "jpg",
            Gif => "gif",
            Bmp => "bmp",
            Webp => "webp",
            Tiff => "tiff",
            Ico => "ico",
            Svg => "svg",
            Heif => "heic",
            Pdf => "pdf",
            Docx => "docx",
            Xlsx => "xlsx",
            Pptx => "pptx",
            OdfText => "odt",
            OdfSheet => "ods",
            OdfPres => "odp",
            Epub => "epub",
            Rtf => "rtf",
            PlainText => "txt",
            Markdown => "md",
            Html => "html",
            Mp4 => "mp4",
            M4a => "m4a",
            QuickTime => "mov",
            Matroska => "mkv",
            WebM => "webm",
            Ogg => "ogg",
            Wav => "wav",
            Flac => "flac",
            Mp3 => "mp3",
            Aac => "aac",
            Avi => "avi",
            Zip => "zip",
            Gzip => "gz",
            Tar => "tar",
            Xz => "xz",
            Zstd => "zst",
            Bzip2 => "bz2",
            SevenZip => "7z",
            Rar => "rar",
            Elf => "elf",
            Pe => "exe",
            Wasm => "wasm",
            Ttf => "ttf",
            Otf => "otf",
            Woff => "woff",
            Woff2 => "woff2",
            Unknown => "",
        }
    }

    /// The coarse [`Category`] for icon / default-app selection.
    pub fn category(self) -> Category {
        use FileKind::*;
        match self {
            Png | Jpeg | Gif | Bmp | Webp | Tiff | Ico | Svg | Heif => Category::Image,
            Pdf | Docx | Xlsx | Pptx | OdfText | OdfSheet | OdfPres | Epub | Rtf => {
                Category::Document
            }
            PlainText | Markdown | Html => Category::Text,
            Ogg | Wav | Flac | Mp3 | Aac | M4a => Category::Audio,
            Mp4 | QuickTime | Matroska | WebM | Avi => Category::Video,
            Zip | Gzip | Tar | Xz | Zstd | Bzip2 | SevenZip | Rar => Category::Archive,
            Elf | Pe | Wasm => Category::Executable,
            Ttf | Otf | Woff | Woff2 => Category::Font,
            Unknown => Category::Other,
        }
    }

    /// The AthenaOS crate name that decodes / handles this kind, if one exists, as
    /// a **string** (NOT a dependency — the caller links the decoder, not us).
    /// `None` for kinds with no in-tree decoder yet.
    pub fn recommended_decoder(self) -> Option<&'static str> {
        use FileKind::*;
        Some(match self {
            Png => "ath_png",
            Jpeg => "ath_jpeg",
            Gif => "ath_gif",
            Bmp => "ath_bmp",
            Pdf => "ath_pdf",
            Docx => "ath_docx",
            Xlsx => "ath_xlsx",
            Mp4 | M4a | QuickTime | Heif => "ath_mp4",
            Zip => "ath_zip",
            Gzip => "ath_deflate",
            Tar => "ath_tar",
            Markdown => "ath_markdown",
            Html => "rae_web",
            _ => return None,
        })
    }

    /// True if this kind is a *container* (an outer wrapper whose true logical
    /// payload an extractor/demuxer discovers): archives and media containers.
    pub fn is_container(self) -> bool {
        use FileKind::*;
        matches!(
            self,
            Zip | Gzip
                | Tar
                | Xz
                | Zstd
                | Bzip2
                | SevenZip
                | Rar
                | Docx
                | Xlsx
                | Pptx
                | OdfText
                | OdfSheet
                | OdfPres
                | Epub
                | Mp4
                | M4a
                | QuickTime
                | Matroska
                | WebM
                | Ogg
                | Avi
                | Heif
        )
    }
}

// ===========================================================================
// Detection
// ===========================================================================

/// Detect a file's true type purely from its leading / structural bytes.
///
/// All detection is bounded byte peeks with explicit length guards; an empty,
/// tiny, truncated, or hostile slice returns [`FileKind::Unknown`] and never
/// panics. Content is the only input here — see [`detect_with_hint`] for the
/// extension-tie-break variant.
pub fn detect(bytes: &[u8]) -> FileKind {
    // Order matters: most-specific / least-collision-prone signatures first, and
    // the ambiguous families are routed to dedicated disambiguators.

    // --- ZIP family (OOXML / ODF / EPUB / JAR / plain zip) ---
    if starts_with(bytes, &[0x50, 0x4B, 0x03, 0x04])
        || starts_with(bytes, &[0x50, 0x4B, 0x05, 0x06]) // empty archive
        || starts_with(bytes, &[0x50, 0x4B, 0x07, 0x08])
    {
        return detect_zip_family(bytes);
    }

    // --- RIFF family (WAV / AVI / WebP) ---
    if starts_with(bytes, b"RIFF") {
        return detect_riff_family(bytes);
    }

    // --- ISO-BMFF / MP4 family (ftyp box at offset 4) ---
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return detect_isobmff(bytes);
    }
    // QuickTime with a leading moov/mdat/free/wide/skip atom at offset 4.
    if bytes.len() >= 8 {
        match &bytes[4..8] {
            b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" => {
                return FileKind::QuickTime
            }
            _ => {}
        }
    }

    // --- EBML (Matroska / WebM) ---
    if starts_with(bytes, &[0x1A, 0x45, 0xDF, 0xA3]) {
        return detect_ebml(bytes);
    }

    // --- single-signature images ---
    if starts_with(bytes, &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return FileKind::Png;
    }
    if starts_with(bytes, &[0xFF, 0xD8, 0xFF]) {
        return FileKind::Jpeg;
    }
    if starts_with(bytes, b"GIF87a") || starts_with(bytes, b"GIF89a") {
        return FileKind::Gif;
    }
    if starts_with(bytes, b"BM") && bytes.len() >= 14 {
        return FileKind::Bmp;
    }
    // TIFF: little-endian "II*\0" or big-endian "MM\0*".
    if starts_with(bytes, &[0x49, 0x49, 0x2A, 0x00])
        || starts_with(bytes, &[0x4D, 0x4D, 0x00, 0x2A])
    {
        return FileKind::Tiff;
    }
    // ICO / CUR: reserved=0, type=1 (icon) at offset 0..4.
    if starts_with(bytes, &[0x00, 0x00, 0x01, 0x00]) {
        return FileKind::Ico;
    }

    // --- documents ---
    if starts_with(bytes, b"%PDF-") {
        return FileKind::Pdf;
    }
    if starts_with(bytes, b"{\\rtf") {
        return FileKind::Rtf;
    }

    // --- archives / compression ---
    if starts_with(bytes, &[0x1F, 0x8B]) {
        return FileKind::Gzip;
    }
    if starts_with(bytes, &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return FileKind::Xz;
    }
    if starts_with(bytes, &[0x28, 0xB5, 0x2F, 0xFD]) {
        return FileKind::Zstd;
    }
    if starts_with(bytes, &[0x42, 0x5A, 0x68]) {
        return FileKind::Bzip2;
    }
    if starts_with(bytes, &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return FileKind::SevenZip;
    }
    if starts_with(bytes, &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07]) {
        return FileKind::Rar; // "Rar!\x1A\x07" (both v4 and v5 share this prefix)
    }
    // ustar magic at offset 257 of a tar header block.
    if bytes.len() >= 262 && &bytes[257..262] == b"ustar" {
        return FileKind::Tar;
    }

    // --- audio (raw streams) ---
    if starts_with(bytes, b"OggS") {
        return FileKind::Ogg;
    }
    if starts_with(bytes, b"fLaC") {
        return FileKind::Flac;
    }
    if starts_with(bytes, b"ID3") {
        return FileKind::Mp3; // MP3 with an ID3v2 tag
    }
    if is_mp3_frame_sync(bytes) {
        return FileKind::Mp3;
    }
    if is_adts_aac(bytes) {
        return FileKind::Aac;
    }

    // --- executables / runtimes / fonts ---
    if starts_with(bytes, &[0x7F, 0x45, 0x4C, 0x46]) {
        return FileKind::Elf;
    }
    if starts_with(bytes, b"MZ") {
        return FileKind::Pe;
    }
    if starts_with(bytes, b"\0asm") {
        return FileKind::Wasm;
    }
    if starts_with(bytes, b"wOF2") {
        return FileKind::Woff2;
    }
    if starts_with(bytes, b"wOFF") {
        return FileKind::Woff;
    }
    if starts_with(bytes, b"OTTO") {
        return FileKind::Otf;
    }
    // TrueType: 0x00010000 (version 1.0) or "true" / "ttcf".
    if starts_with(bytes, &[0x00, 0x01, 0x00, 0x00])
        || starts_with(bytes, b"true")
        || starts_with(bytes, b"ttcf")
    {
        return FileKind::Ttf;
    }

    // --- text-ish heuristics (last, since they overlap real text) ---
    if let Some(kind) = detect_text_markup(bytes) {
        return kind;
    }
    if looks_like_text(bytes) {
        return FileKind::PlainText;
    }

    FileKind::Unknown
}

/// Detect with an optional filename / extension hint. **Content always wins**; the
/// hint only (a) breaks a genuine tie that content cannot resolve, and (b) refines
/// text into a more specific text type (`.md` → Markdown, `.html` → Html) or
/// rescues an extension-less plain-text file. A mismatched extension never
/// overrides a clear magic match — a PNG body named `notes.txt` is still Png.
pub fn detect_with_hint(bytes: &[u8], filename_or_ext: Option<&str>) -> FileKind {
    let by_content = detect(bytes);

    let ext = filename_or_ext.map(extract_ext);

    match by_content {
        // Plain text is the one content verdict the extension may legitimately
        // refine into a more specific text type (the bytes alone can't tell a
        // Markdown file from a .txt with the same characters).
        FileKind::PlainText => {
            if let Some(e) = ext {
                if let Some(k) = text_kind_from_ext(&e) {
                    return k;
                }
            }
            FileKind::PlainText
        }
        // Content was inconclusive: fall back to the extension entirely.
        FileKind::Unknown => {
            if let Some(e) = ext {
                if let Some(k) = kind_from_ext(&e) {
                    return k;
                }
            }
            FileKind::Unknown
        }
        // Any confident content verdict stands — content is truth.
        other => other,
    }
}

// ---------------------------------------------------------------------------
// ZIP-family disambiguation
// ---------------------------------------------------------------------------

/// Disambiguate a ZIP container by peeking its first local-file-header entry name
/// (and, for ODF/EPUB, the stored body of a `mimetype` entry) WITHOUT unzipping.
///
/// Local file header layout (offset 0 of the entry):
///   0  u32  signature  PK\x03\x04
///   ..
///   18 u32  compressed size
///   22 u32  uncompressed size
///   26 u16  file name length (n)
///   28 u16  extra field length (m)
///   30 [n]  file name
///   30+n [m] extra
///   30+n+m [..] file data (compressed size bytes)
fn detect_zip_family(bytes: &[u8]) -> FileKind {
    // Only the 0x03 0x04 local-file-header form carries a name; the empty/spanned
    // forms (0506 / 0708) are just generic ZIPs.
    if !starts_with(bytes, &[0x50, 0x4B, 0x03, 0x04]) {
        return FileKind::Zip;
    }
    // Need the fixed 30-byte local header.
    if bytes.len() < 30 {
        return FileKind::Zip;
    }
    let comp_size = le_u32(bytes, 18) as usize;
    let name_len = le_u16(bytes, 26) as usize;
    let extra_len = le_u16(bytes, 28) as usize;

    let name_start = 30usize;
    let name_end = match name_start.checked_add(name_len) {
        Some(v) if v <= bytes.len() => v,
        _ => return FileKind::Zip, // name runs past the buffer → can't refine
    };
    let name = &bytes[name_start..name_end];

    // OOXML: the first entry is conventionally `[Content_Types].xml`, but the
    // payload tree (`word/`, `xl/`, `ppt/`) is the reliable discriminator and
    // many writers put a payload dir first. Check the name's leading path segment.
    if name_has_prefix(name, b"word/") {
        return FileKind::Docx;
    }
    if name_has_prefix(name, b"xl/") {
        return FileKind::Xlsx;
    }
    if name_has_prefix(name, b"ppt/") {
        return FileKind::Pptx;
    }

    // ODF / EPUB: the first entry MUST be an (uncompressed, stored) `mimetype`
    // file whose body is the package MIME string. Read that body in place.
    if name == b"mimetype" {
        let body_start = name_end.saturating_add(extra_len);
        let body_end = match body_start.checked_add(comp_size) {
            Some(v) if v <= bytes.len() => v,
            // size field unreliable / truncated: scan a bounded window instead.
            _ => core::cmp::min(bytes.len(), body_start.saturating_add(64)),
        };
        if body_start <= body_end && body_start <= bytes.len() {
            let body = &bytes[body_start..body_end];
            return classify_mimetype_body(body);
        }
        return FileKind::Zip;
    }

    // Some OOXML files lead with `[Content_Types].xml`; we can't tell doc vs sheet
    // vs pres from that name alone without reading the central directory, so we
    // leave it as a generic Zip (the extension hint, if any, refines it). This is
    // the honest, never-over-claim behavior.
    FileKind::Zip
}

/// Classify an ODF/EPUB `mimetype` body string into the specific kind.
fn classify_mimetype_body(body: &[u8]) -> FileKind {
    if contains(body, b"application/epub+zip") {
        return FileKind::Epub;
    }
    if contains(body, b"opendocument.text") {
        return FileKind::OdfText;
    }
    if contains(body, b"opendocument.spreadsheet") {
        return FileKind::OdfSheet;
    }
    if contains(body, b"opendocument.presentation") {
        return FileKind::OdfPres;
    }
    FileKind::Zip
}

// ---------------------------------------------------------------------------
// RIFF-family disambiguation
// ---------------------------------------------------------------------------

/// `RIFF....FORM` — the 4-byte form type at offset 8 picks WAV / AVI / WebP.
fn detect_riff_family(bytes: &[u8]) -> FileKind {
    if bytes.len() < 12 {
        return FileKind::Unknown; // not enough to read the form type
    }
    match &bytes[8..12] {
        b"WAVE" => FileKind::Wav,
        b"AVI " => FileKind::Avi,
        b"WEBP" => FileKind::Webp,
        _ => FileKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// ISO-BMFF / MP4 brand disambiguation
// ---------------------------------------------------------------------------

/// `....ftypBRND` — the major brand (offset 8..12) picks video vs audio vs HEIF
/// vs QuickTime. Bounded; a too-short slice falls back to generic Mp4.
fn detect_isobmff(bytes: &[u8]) -> FileKind {
    if bytes.len() < 12 {
        return FileKind::Mp4;
    }
    let brand = &bytes[8..12];
    match brand {
        b"qt  " => FileKind::QuickTime,
        b"M4A " | b"M4B " | b"M4P " => FileKind::M4a,
        b"heic" | b"heix" | b"heif" | b"mif1" | b"msf1" | b"hevc" => FileKind::Heif,
        b"M4V " | b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1" | b"dash" => FileKind::Mp4,
        // Unknown brand but it IS an ftyp box → most likely MP4-ish video.
        _ => FileKind::Mp4,
    }
}

// ---------------------------------------------------------------------------
// EBML (Matroska / WebM) disambiguation
// ---------------------------------------------------------------------------

/// EBML header `1A45DFA3` → Matroska/WebM. A bounded scan of the EBML header for
/// the `webm` DocType string distinguishes WebM from generic Matroska.
fn detect_ebml(bytes: &[u8]) -> FileKind {
    // The DocType element (ID 0x4282) sits early in the EBML header. Rather than
    // decode the variable-length EBML element tree, scan a bounded leading window
    // for the literal DocType strings — both are short ASCII and appear verbatim.
    let window = &bytes[..core::cmp::min(bytes.len(), 1024)];
    if contains(window, b"webm") {
        return FileKind::WebM;
    }
    if contains(window, b"matroska") {
        return FileKind::Matroska;
    }
    // EBML magic present but DocType not found in the window → default to Matroska.
    FileKind::Matroska
}

// ---------------------------------------------------------------------------
// MP3 / AAC frame-sync heuristics
// ---------------------------------------------------------------------------

/// A raw (tagless) MP3 begins with an MPEG-audio frame sync: 11 set bits
/// (`0xFF` then top 3 bits of the next byte set), with a valid MPEG version
/// (not the reserved `01`) and a valid layer (not the reserved `00`).
fn is_mp3_frame_sync(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    let b0 = bytes[0];
    let b1 = bytes[1];
    if b0 != 0xFF || (b1 & 0xE0) != 0xE0 {
        return false; // no 11-bit sync
    }
    let version = (b1 >> 3) & 0x03; // 01 = reserved
    let layer = (b1 >> 1) & 0x03; // 00 = reserved
    version != 0b01 && layer != 0b00
}

/// ADTS AAC: a 12-bit sync `0xFFF` then a layer field of `00` (MPEG-4/2 ADTS).
fn is_adts_aac(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    let b0 = bytes[0];
    let b1 = bytes[1];
    // 0xFFF sync = 0xFF then top 4 bits of b1 are 1111? ADTS sync is 0xFFF =
    // b0==0xFF and (b1 & 0xF6)==0xF0: 4 sync bits + MPEG ver(1) + layer(00).
    b0 == 0xFF && (b1 & 0xF6) == 0xF0
}

// ---------------------------------------------------------------------------
// Text / markup heuristics
// ---------------------------------------------------------------------------

/// Detect SVG / HTML from a (possibly BOM-prefixed) text prefix. Returns `None`
/// if the bytes are not recognizably one of these markup languages.
fn detect_text_markup(bytes: &[u8]) -> Option<FileKind> {
    let s = leading_text(bytes, 512)?;
    let lower = ascii_lower_owned(s);
    let l = lower.as_str();
    // Skip leading whitespace for the structural checks.
    let trimmed = l.trim_start();
    if trimmed.starts_with("<svg") || (trimmed.starts_with("<?xml") && l.contains("<svg")) {
        return Some(FileKind::Svg);
    }
    if trimmed.starts_with("<!doctype html") || trimmed.starts_with("<html") {
        return Some(FileKind::Html);
    }
    None
}

/// Heuristic: does the leading window look like printable text (UTF-8-ish, no NULs
/// and a high ratio of printable bytes)? Used as the final fallback before
/// `Unknown` so a plain `.txt`/source file is classified as PlainText.
fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let window = &bytes[..core::cmp::min(bytes.len(), 512)];
    let mut printable = 0usize;
    for &b in window {
        if b == 0 {
            return false; // a NUL byte means binary
        }
        // tab, LF, CR, or printable ASCII; allow high bytes (UTF-8 continuation).
        if b == 0x09 || b == 0x0A || b == 0x0D || (0x20..=0x7E).contains(&b) || b >= 0x80 {
            printable += 1;
        }
    }
    // Require an overwhelming majority printable.
    printable * 100 >= window.len() * 95
}

// ---------------------------------------------------------------------------
// Extension-hint mapping (no ath_mime dependency — kept inline & minimal)
// ---------------------------------------------------------------------------

/// Map a more-specific *text* extension onto its [`FileKind`] (used only to refine
/// a PlainText content verdict).
fn text_kind_from_ext(ext: &str) -> Option<FileKind> {
    Some(match ext {
        "md" | "markdown" | "mdown" => FileKind::Markdown,
        "html" | "htm" | "xhtml" => FileKind::Html,
        "svg" => FileKind::Svg,
        "txt" | "text" | "log" | "ini" | "cfg" | "conf" | "csv" | "tsv" | "json" | "toml"
        | "xml" | "yaml" | "yml" | "rs" | "c" | "h" | "py" | "sh" | "js" | "css" => {
            FileKind::PlainText
        }
        _ => return None,
    })
}

/// Map any known extension onto a [`FileKind`] (used only when content detection
/// is fully inconclusive — `Unknown`). This is intentionally small and never
/// overrides content.
fn kind_from_ext(ext: &str) -> Option<FileKind> {
    if let Some(k) = text_kind_from_ext(ext) {
        return Some(k);
    }
    Some(match ext {
        "png" => FileKind::Png,
        "jpg" | "jpeg" | "jpe" => FileKind::Jpeg,
        "gif" => FileKind::Gif,
        "bmp" => FileKind::Bmp,
        "webp" => FileKind::Webp,
        "tif" | "tiff" => FileKind::Tiff,
        "ico" => FileKind::Ico,
        "heic" | "heif" => FileKind::Heif,
        "pdf" => FileKind::Pdf,
        "docx" => FileKind::Docx,
        "xlsx" => FileKind::Xlsx,
        "pptx" => FileKind::Pptx,
        "odt" => FileKind::OdfText,
        "ods" => FileKind::OdfSheet,
        "odp" => FileKind::OdfPres,
        "epub" => FileKind::Epub,
        "rtf" => FileKind::Rtf,
        "mp4" | "m4v" => FileKind::Mp4,
        "m4a" | "m4b" => FileKind::M4a,
        "mov" => FileKind::QuickTime,
        "mkv" => FileKind::Matroska,
        "webm" => FileKind::WebM,
        "ogg" | "oga" | "opus" => FileKind::Ogg,
        "wav" => FileKind::Wav,
        "flac" => FileKind::Flac,
        "mp3" => FileKind::Mp3,
        "aac" => FileKind::Aac,
        "avi" => FileKind::Avi,
        "zip" => FileKind::Zip,
        "gz" | "gzip" | "tgz" => FileKind::Gzip,
        "tar" => FileKind::Tar,
        "xz" | "txz" => FileKind::Xz,
        "zst" | "zstd" => FileKind::Zstd,
        "bz2" | "tbz" => FileKind::Bzip2,
        "7z" => FileKind::SevenZip,
        "rar" => FileKind::Rar,
        "elf" => FileKind::Elf,
        "exe" | "dll" => FileKind::Pe,
        "wasm" => FileKind::Wasm,
        "ttf" | "ttc" => FileKind::Ttf,
        "otf" => FileKind::Otf,
        "woff" => FileKind::Woff,
        "woff2" => FileKind::Woff2,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Bounded byte-peek helpers (all length-guarded; none can panic)
// ---------------------------------------------------------------------------

#[inline]
fn starts_with(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && &bytes[..prefix.len()] == prefix
}

/// True if `name`'s first path segment equals `prefix` (e.g. `word/`). `prefix`
/// must itself end in `/`. Never panics.
#[inline]
fn name_has_prefix(name: &[u8], prefix: &[u8]) -> bool {
    starts_with(name, prefix)
}

/// Read a little-endian u16 at `off`, or 0 if out of range.
#[inline]
fn le_u16(bytes: &[u8], off: usize) -> u16 {
    match bytes.get(off..off + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]),
        None => 0,
    }
}

/// Read a little-endian u32 at `off`, or 0 if out of range.
#[inline]
fn le_u32(bytes: &[u8], off: usize) -> u32 {
    match bytes.get(off..off + 4) {
        Some(b) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}

/// Bounded substring search (`haystack.contains(needle)` for byte slices). Empty
/// needle → false (a degenerate match is never useful here). Never panics.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let last = haystack.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if &haystack[i..i + needle.len()] == needle {
            return true;
        }
        i += 1;
    }
    false
}

/// Borrow the leading up-to-`max` bytes as `&str` if they are valid UTF-8 (after
/// skipping a UTF-8 BOM). Returns `None` if not valid UTF-8. Never panics.
fn leading_text(bytes: &[u8], max: usize) -> Option<&str> {
    let start: usize = if starts_with(bytes, &[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let end = core::cmp::min(bytes.len(), start.saturating_add(max));
    let slice = bytes.get(start..end)?;
    core::str::from_utf8(slice).ok()
}

/// Lowercase ASCII into an owned `String` (non-ASCII passes through). Never panics.
fn ascii_lower_owned(s: &str) -> alloc::string::String {
    let mut out = alloc::string::String::with_capacity(s.len());
    for &b in s.as_bytes() {
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// Extract the lowercase dot-less extension from a filename or a bare extension.
/// `"photo.PNG"` → `"png"`; `"png"` → `"png"`; `".bashrc"` → `""`. Never panics.
fn extract_ext(name_or_ext: &str) -> alloc::string::String {
    // basename
    let bytes = name_or_ext.as_bytes();
    let mut start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'/' || b == b'\\' {
            start = i + 1;
        }
    }
    let base = if start >= name_or_ext.len() {
        ""
    } else {
        &name_or_ext[start..]
    };
    // last dot, not the first char
    let bb = base.as_bytes();
    let mut dot = None;
    for (i, &b) in bb.iter().enumerate() {
        if b == b'.' && i > 0 {
            dot = Some(i);
        }
    }
    let ext = match dot {
        Some(d) => &base[d + 1..],
        // No dot at all → treat the whole thing as a bare extension.
        None => base,
    };
    ascii_lower_owned(ext)
}

// ===========================================================================
// Host KATs — the primary, FAIL-able proof. `cargo test -p ath_formats`.
//
// FAIL-ability: every assertion compares against an explicit expected FileKind
// (e.g. `detect(&png) == FileKind::Png`) AND, for the table-driven cases, asserts
// it is NOT a plausible wrong kind. Break a signature in `detect` and the matching
// row fails immediately. The disambiguation tests (docx-vs-zip, RIFF family,
// ftyp brands, EBML, ID3-vs-framesync) assert the EXACT winner.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the no_std attribute is
// cfg_attr(not(test), ...)), so `Vec`/`String` are in the default prelude — no
// `extern crate std` / `use std::` lines (the architecture gate bans those).
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    // ---- helpers to forge minimal real headers ----

    /// A minimal ZIP local file header: PK\x03\x04, then the fixed 26 bytes, with
    /// `name` as the first entry and `body` as its stored data.
    fn zip_with_first_entry(name: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // signature
        v.extend_from_slice(&[0x14, 0x00]); // version needed
        v.extend_from_slice(&[0x00, 0x00]); // flags
        v.extend_from_slice(&[0x00, 0x00]); // method (stored)
        v.extend_from_slice(&[0x00, 0x00]); // mod time
        v.extend_from_slice(&[0x00, 0x00]); // mod date
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // crc32
        v.extend_from_slice(&(body.len() as u32).to_le_bytes()); // compressed size
        v.extend_from_slice(&(body.len() as u32).to_le_bytes()); // uncompressed size
        v.extend_from_slice(&(name.len() as u16).to_le_bytes()); // name len
        v.extend_from_slice(&[0x00, 0x00]); // extra len
        v.extend_from_slice(name);
        v.extend_from_slice(body);
        v
    }

    fn riff(form: &[u8; 4]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&[0x24, 0x00, 0x00, 0x00]); // some size
        v.extend_from_slice(form);
        v.extend_from_slice(&[0u8; 8]); // a little payload
        v
    }

    fn ftyp(brand: &[u8; 4]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x18]); // box size
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(brand); // major brand
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // minor version
        v.extend_from_slice(brand); // a compatible brand
        v.extend_from_slice(&[0u8; 4]);
        v
    }

    fn ebml(doctype: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]); // EBML magic
        v.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // filler
        v.extend_from_slice(doctype); // DocType string somewhere in the header
        v.extend_from_slice(&[0u8; 16]);
        v
    }

    // ====================================================================
    // 1. Table-driven single-signature KATs (each proven FAIL-able by also
    //    asserting it is NOT a sibling wrong kind).
    // ====================================================================

    #[test]
    fn single_signature_table() {
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01];
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let gif = b"GIF89a\x01\x00".to_vec();
        let bmp = {
            let mut v = b"BM".to_vec();
            v.extend_from_slice(&[0u8; 20]);
            v
        };
        let pdf = b"%PDF-1.7\n".to_vec();
        let gz = vec![0x1F, 0x8B, 0x08, 0x00];
        let xz = vec![0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00];
        let zst = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00];
        let bz = b"BZh9".to_vec();
        let sevenz = vec![0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00];
        let rar = b"Rar!\x1A\x07\x00".to_vec();
        let elf = vec![0x7F, 0x45, 0x4C, 0x46, 0x02, 0x01];
        let pe = b"MZ\x90\x00".to_vec();
        let wasm = b"\0asm\x01\x00\x00\x00".to_vec();
        let ogg = b"OggS\x00\x02".to_vec();
        let flac = b"fLaC\x00\x00".to_vec();
        let tiff_le = vec![0x49, 0x49, 0x2A, 0x00];
        let tiff_be = vec![0x4D, 0x4D, 0x00, 0x2A];
        let ico = vec![0x00, 0x00, 0x01, 0x00, 0x01, 0x00];
        let rtf = b"{\\rtf1\\ansi".to_vec();
        let woff2 = b"wOF2\x00\x01".to_vec();
        let woff = b"wOFF\x00\x01".to_vec();
        let otf = b"OTTO\x00\x00".to_vec();
        let ttf = vec![0x00, 0x01, 0x00, 0x00, 0x00];

        // (bytes, expected, a wrong sibling that must NOT match)
        let cases: &[(&[u8], FileKind, FileKind)] = &[
            (&png, FileKind::Png, FileKind::Jpeg),
            (&jpeg, FileKind::Jpeg, FileKind::Png),
            (&gif, FileKind::Gif, FileKind::Png),
            (&bmp, FileKind::Bmp, FileKind::Png),
            (&pdf, FileKind::Pdf, FileKind::PlainText),
            (&gz, FileKind::Gzip, FileKind::Zip),
            (&xz, FileKind::Xz, FileKind::Gzip),
            (&zst, FileKind::Zstd, FileKind::Gzip),
            (&bz, FileKind::Bzip2, FileKind::Gzip),
            (&sevenz, FileKind::SevenZip, FileKind::Zip),
            (&rar, FileKind::Rar, FileKind::Zip),
            (&elf, FileKind::Elf, FileKind::Pe),
            (&pe, FileKind::Pe, FileKind::Elf),
            (&wasm, FileKind::Wasm, FileKind::Elf),
            (&ogg, FileKind::Ogg, FileKind::Flac),
            (&flac, FileKind::Flac, FileKind::Ogg),
            (&tiff_le, FileKind::Tiff, FileKind::Png),
            (&tiff_be, FileKind::Tiff, FileKind::Png),
            (&ico, FileKind::Ico, FileKind::Png),
            (&rtf, FileKind::Rtf, FileKind::PlainText),
            (&woff2, FileKind::Woff2, FileKind::Woff),
            (&woff, FileKind::Woff, FileKind::Woff2),
            (&otf, FileKind::Otf, FileKind::Ttf),
            (&ttf, FileKind::Ttf, FileKind::Otf),
        ];
        for (bytes, want, wrong) in cases {
            assert_eq!(detect(bytes), *want, "detect mismatch for {:?}", want);
            assert_ne!(detect(bytes), *wrong, "false-matched sibling {:?}", wrong);
        }
    }

    #[test]
    fn tar_ustar_at_offset_257() {
        let mut block = vec![0u8; 270];
        block[257..262].copy_from_slice(b"ustar");
        assert_eq!(detect(&block), FileKind::Tar);
        // FAIL-able: a buffer too short to reach offset 257 must NOT be Tar.
        assert_ne!(detect(b"ustar"), FileKind::Tar);
    }

    // ====================================================================
    // 2. THE HARD ONE: ZIP-family disambiguation (docx/xlsx/pptx/odf/epub
    //    vs generic zip) by peeking the first local-file-entry name.
    // ====================================================================

    #[test]
    fn zip_family_ooxml_disambiguation() {
        let docx = zip_with_first_entry(b"word/document.xml", b"<xml/>");
        let xlsx = zip_with_first_entry(b"xl/workbook.xml", b"<xml/>");
        let pptx = zip_with_first_entry(b"ppt/presentation.xml", b"<xml/>");

        assert_eq!(detect(&docx), FileKind::Docx);
        assert_eq!(detect(&xlsx), FileKind::Xlsx);
        assert_eq!(detect(&pptx), FileKind::Pptx);

        // THE proof that disambiguation actually happened: these are NOT generic Zip.
        assert_ne!(detect(&docx), FileKind::Zip);
        assert_ne!(detect(&xlsx), FileKind::Zip);
        assert_ne!(detect(&pptx), FileKind::Zip);
        // ...and not confused with each other.
        assert_ne!(detect(&docx), FileKind::Xlsx);
        assert_ne!(detect(&xlsx), FileKind::Pptx);
    }

    #[test]
    fn zip_family_odf_and_epub_via_mimetype() {
        let odt = zip_with_first_entry(b"mimetype", b"application/vnd.oasis.opendocument.text");
        let ods = zip_with_first_entry(
            b"mimetype",
            b"application/vnd.oasis.opendocument.spreadsheet",
        );
        let odp = zip_with_first_entry(
            b"mimetype",
            b"application/vnd.oasis.opendocument.presentation",
        );
        let epub = zip_with_first_entry(b"mimetype", b"application/epub+zip");

        assert_eq!(detect(&odt), FileKind::OdfText);
        assert_eq!(detect(&ods), FileKind::OdfSheet);
        assert_eq!(detect(&odp), FileKind::OdfPres);
        assert_eq!(detect(&epub), FileKind::Epub);

        // FAIL-able: a `mimetype` body we don't recognize stays generic Zip.
        let weird = zip_with_first_entry(b"mimetype", b"application/x-something-else");
        assert_eq!(detect(&weird), FileKind::Zip);
    }

    #[test]
    fn zip_generic_and_unknown_first_entry() {
        // A plain zip whose first entry is just a file → generic Zip.
        let plain = zip_with_first_entry(b"readme.txt", b"hello");
        assert_eq!(detect(&plain), FileKind::Zip);
        // The empty-archive end-of-central-directory signature → generic Zip.
        let empty = vec![0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0];
        assert_eq!(detect(&empty), FileKind::Zip);
        // A truncated local header (name claims more than is present) → Zip, no panic.
        let mut trunc = zip_with_first_entry(b"word/document.xml", b"x");
        trunc.truncate(34); // cut into the name
        let _ = detect(&trunc); // must not panic
    }

    // ====================================================================
    // 3. RIFF family — same first 4 bytes, different kind.
    // ====================================================================

    #[test]
    fn riff_family_disambiguation() {
        let wav = riff(b"WAVE");
        let avi = riff(b"AVI ");
        let webp = riff(b"WEBP");
        assert_eq!(detect(&wav), FileKind::Wav);
        assert_eq!(detect(&avi), FileKind::Avi);
        assert_eq!(detect(&webp), FileKind::Webp);
        // All three share the RIFF prefix — prove they are NOT collapsed together.
        assert_ne!(detect(&wav), detect(&avi));
        assert_ne!(detect(&avi), detect(&webp));
        assert_ne!(detect(&wav), detect(&webp));
        // A RIFF with an unknown form type → Unknown (not a wrong guess).
        let mystery = riff(b"XXXX");
        assert_eq!(detect(&mystery), FileKind::Unknown);
        // A bare "RIFF" with no form type → no panic, Unknown.
        assert_eq!(detect(b"RIFF"), FileKind::Unknown);
    }

    // ====================================================================
    // 4. ISO-BMFF brand disambiguation (mp4 video vs m4a audio vs heif/qt).
    // ====================================================================

    #[test]
    fn isobmff_brand_disambiguation() {
        assert_eq!(detect(&ftyp(b"isom")), FileKind::Mp4);
        assert_eq!(detect(&ftyp(b"mp42")), FileKind::Mp4);
        assert_eq!(detect(&ftyp(b"avc1")), FileKind::Mp4);
        assert_eq!(detect(&ftyp(b"M4A ")), FileKind::M4a);
        assert_eq!(detect(&ftyp(b"M4B ")), FileKind::M4a);
        assert_eq!(detect(&ftyp(b"qt  ")), FileKind::QuickTime);
        assert_eq!(detect(&ftyp(b"heic")), FileKind::Heif);
        assert_eq!(detect(&ftyp(b"mif1")), FileKind::Heif);

        // The mp4-vs-m4a proof: same ftyp box machinery, brand decides.
        assert_ne!(detect(&ftyp(b"isom")), detect(&ftyp(b"M4A ")));
        // A QuickTime leading-atom form (moov at offset 4) is detected too.
        let mut qt = vec![0x00, 0x00, 0x00, 0x10];
        qt.extend_from_slice(b"moov");
        qt.extend_from_slice(&[0u8; 8]);
        assert_eq!(detect(&qt), FileKind::QuickTime);
        // Truncated ftyp (no brand) → generic Mp4, no panic.
        let mut short = vec![0u8, 0, 0, 0];
        short.extend_from_slice(b"ftyp");
        assert_eq!(detect(&short), FileKind::Mp4);
    }

    // ====================================================================
    // 5. EBML — Matroska vs WebM via DocType heuristic.
    // ====================================================================

    #[test]
    fn ebml_matroska_vs_webm() {
        let mkv = ebml(b"\x42\x82\x88matroska");
        let webm = ebml(b"\x42\x82\x84webm");
        assert_eq!(detect(&mkv), FileKind::Matroska);
        assert_eq!(detect(&webm), FileKind::WebM);
        assert_ne!(detect(&mkv), detect(&webm));
        // EBML magic with no DocType in the window → defaults to Matroska, no panic.
        let bare = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00];
        assert_eq!(detect(&bare), FileKind::Matroska);
    }

    // ====================================================================
    // 6. MP3: ID3 tag vs raw frame sync; AAC ADTS.
    // ====================================================================

    #[test]
    fn mp3_id3_and_frame_sync() {
        let id3 = b"ID3\x03\x00\x00\x00\x00".to_vec();
        assert_eq!(detect(&id3), FileKind::Mp3);

        // Raw frame sync: 0xFF 0xFB = sync(11 bits) + MPEG1 + Layer III.
        let frame = vec![0xFF, 0xFB, 0x90, 0x00];
        assert_eq!(detect(&frame), FileKind::Mp3);

        // FAIL-able: a reserved version (0xFF 0xE9 → version bits 01) is NOT mp3.
        let reserved_ver = vec![0xFF, 0xE9, 0x00];
        assert_ne!(detect(&reserved_ver), FileKind::Mp3);

        // ADTS AAC: 0xFF 0xF1 (sync 0xFFF + MPEG-4 + layer 00 + protection absent).
        let aac = vec![0xFF, 0xF1, 0x50, 0x80];
        assert_eq!(detect(&aac), FileKind::Aac);
    }

    // ====================================================================
    // 7. Text / markup heuristics.
    // ====================================================================

    #[test]
    fn text_markup_heuristics() {
        assert_eq!(detect(b"<svg xmlns=\"...\"></svg>"), FileKind::Svg);
        assert_eq!(
            detect(b"<?xml version=\"1.0\"?>\n<svg></svg>"),
            FileKind::Svg
        );
        assert_eq!(detect(b"<!DOCTYPE html><html></html>"), FileKind::Html);
        assert_eq!(detect(b"<html lang=\"en\">"), FileKind::Html);
        // Plain ASCII text → PlainText.
        assert_eq!(detect(b"just some plain notes here\n"), FileKind::PlainText);
        // A BOM-prefixed text file is still text.
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"hello world");
        assert_eq!(detect(&bom), FileKind::PlainText);
        // Binary (has a NUL) is NOT mistaken for text.
        assert_eq!(detect(b"abc\0def\0ghi"), FileKind::Unknown);
    }

    // ====================================================================
    // 8. Extension hint: content wins; hint only breaks genuine ties.
    // ====================================================================

    #[test]
    fn extension_does_not_override_clear_magic() {
        // A PNG body named ".txt" must STILL be Png — the core security property.
        let png = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00];
        assert_eq!(detect_with_hint(&png, Some("photo.txt")), FileKind::Png);
        assert_eq!(detect_with_hint(&png, Some("evil.exe")), FileKind::Png);
        // An ELF named ".png" is still an executable (the #6 security case).
        let elf = vec![0x7F, 0x45, 0x4C, 0x46, 0x02];
        assert_eq!(
            detect_with_hint(&elf, Some("cute_kitten.png")),
            FileKind::Elf
        );
    }

    #[test]
    fn extension_refines_plain_text() {
        let md = b"# Heading\n\nsome *markdown* body\n";
        // Without a hint, structural-less markdown reads as PlainText...
        assert_eq!(detect(md), FileKind::PlainText);
        // ...with a .md hint it refines to Markdown (a true tie content can't break).
        assert_eq!(detect_with_hint(md, Some("notes.md")), FileKind::Markdown);
        // A .txt hint keeps it PlainText.
        assert_eq!(detect_with_hint(md, Some("notes.txt")), FileKind::PlainText);
        // An HTML-extension hint on plain text refines to Html only via hint —
        // but real HTML content already detects as Html without a hint (above).
        assert_eq!(
            detect_with_hint(b"hello", Some("page.html")),
            FileKind::Html
        );
    }

    #[test]
    fn extension_used_only_when_content_unknown() {
        // Truly unknown content + a known extension → use the extension.
        let blob = vec![0x12u8, 0x34, 0x56, 0x78];
        assert_eq!(detect(&blob), FileKind::Unknown);
        assert_eq!(detect_with_hint(&blob, Some("clip.mp4")), FileKind::Mp4);
        // Unknown content + unknown extension → Unknown.
        assert_eq!(detect_with_hint(&blob, Some("x.zzz")), FileKind::Unknown);
        // Unknown content + no hint → Unknown.
        assert_eq!(detect_with_hint(&blob, None), FileKind::Unknown);
    }

    // ====================================================================
    // 9. FileKind metadata accessors.
    // ====================================================================

    #[test]
    fn metadata_accessors() {
        assert_eq!(FileKind::Png.mime_type(), "image/png");
        assert_eq!(FileKind::Png.extension(), "png");
        assert_eq!(FileKind::Png.category(), Category::Image);
        assert_eq!(FileKind::Png.recommended_decoder(), Some("ath_png"));
        assert!(!FileKind::Png.is_container());

        assert_eq!(FileKind::Docx.category(), Category::Document);
        assert!(FileKind::Docx.is_container());
        assert_eq!(
            FileKind::Docx.mime_type(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );

        assert_eq!(FileKind::Mp4.category(), Category::Video);
        assert!(FileKind::Mp4.is_container());
        assert_eq!(FileKind::Mp3.category(), Category::Audio);
        assert_eq!(FileKind::Elf.category(), Category::Executable);
        assert_eq!(FileKind::Ttf.category(), Category::Font);
        assert_eq!(FileKind::Unknown.mime_type(), "application/octet-stream");
        assert_eq!(FileKind::Unknown.recommended_decoder(), None);
        assert_eq!(FileKind::Unknown.category(), Category::Other);

        // Every kind's mime has a '/' and a non-empty extension except Unknown.
        for k in ALL_KINDS {
            assert!(k.mime_type().contains('/'), "{:?} mime lacks /", k);
            if *k != FileKind::Unknown {
                assert!(!k.extension().is_empty(), "{:?} has no extension", k);
            }
        }
    }

    const ALL_KINDS: &[FileKind] = &[
        FileKind::Png,
        FileKind::Jpeg,
        FileKind::Gif,
        FileKind::Bmp,
        FileKind::Webp,
        FileKind::Tiff,
        FileKind::Ico,
        FileKind::Svg,
        FileKind::Heif,
        FileKind::Pdf,
        FileKind::Docx,
        FileKind::Xlsx,
        FileKind::Pptx,
        FileKind::OdfText,
        FileKind::OdfSheet,
        FileKind::OdfPres,
        FileKind::Epub,
        FileKind::Rtf,
        FileKind::PlainText,
        FileKind::Markdown,
        FileKind::Html,
        FileKind::Mp4,
        FileKind::M4a,
        FileKind::QuickTime,
        FileKind::Matroska,
        FileKind::WebM,
        FileKind::Ogg,
        FileKind::Wav,
        FileKind::Flac,
        FileKind::Mp3,
        FileKind::Aac,
        FileKind::Avi,
        FileKind::Zip,
        FileKind::Gzip,
        FileKind::Tar,
        FileKind::Xz,
        FileKind::Zstd,
        FileKind::Bzip2,
        FileKind::SevenZip,
        FileKind::Rar,
        FileKind::Elf,
        FileKind::Pe,
        FileKind::Wasm,
        FileKind::Ttf,
        FileKind::Otf,
        FileKind::Woff,
        FileKind::Woff2,
        FileKind::Unknown,
    ];

    // ====================================================================
    // 10. Never-panic on empty / tiny / truncated input.
    // ====================================================================

    #[test]
    fn empty_tiny_truncated_never_panic() {
        assert_eq!(detect(&[]), FileKind::Unknown);
        assert_eq!(detect(&[0x00]), FileKind::Unknown);
        // Every 1-byte value must not panic.
        for b in 0u16..=255 {
            let _ = detect(&[b as u8]);
        }
        // Truncations of each known signature: never panic.
        let sigs: &[&[u8]] = &[
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            b"RIFF\x00\x00\x00\x00WAVE",
            &[0x1A, 0x45, 0xDF, 0xA3],
            b"\x00\x00\x00\x18ftypisom",
            &[0x50, 0x4B, 0x03, 0x04],
        ];
        for sig in sigs {
            for cut in 0..=sig.len() {
                let _ = detect(&sig[..cut]); // must not panic
                let _ = detect_with_hint(&sig[..cut], Some("x.bin"));
            }
        }
    }

    // ====================================================================
    // 11. Seeded fuzz — random + mutated-valid headers, bounded, panic-free.
    //
    // FAIL-ability: `#![forbid(unsafe_code)]` makes any OOB index a guaranteed
    // panic (not silent UB), so a buffer that drove any detection path out of
    // bounds would abort the test process. Surviving 100k+ adversarial inputs is
    // the never-panic proof.
    // ====================================================================

    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
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
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = Rng::new(0xF0E1_D2C3_B4A5_9687);
        for _ in 0..60_000 {
            let len = rng.below(400);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = detect(&buf);
            let _ = detect_with_hint(&buf, Some("file.bin"));
        }
        // pathological constant buffers at every boundary length
        for len in 0..=300usize {
            let _ = detect(&vec![0xFFu8; len]);
            let _ = detect(&vec![0x00u8; len]);
            let _ = detect(&vec![0x50u8; len]); // 'P' — partial PK
            let _ = detect(&vec![b'R'; len]); // partial RIFF
        }
    }

    #[test]
    fn fuzz_mutated_valid_headers_never_panic() {
        let seeds: &[Vec<u8>] = &[
            zip_with_first_entry(b"word/document.xml", b"<x/>"),
            zip_with_first_entry(b"mimetype", b"application/epub+zip"),
            riff(b"WAVE"),
            riff(b"WEBP"),
            ftyp(b"isom"),
            ftyp(b"M4A "),
            ebml(b"\x42\x82\x84webm"),
            b"%PDF-1.7\n".to_vec(),
            vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        ];
        let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
        for seed in seeds {
            // Each seed must classify (not Unknown) when intact.
            assert_ne!(detect(seed), FileKind::Unknown, "seed should classify");
            for _ in 0..8_000 {
                let mut m = seed.clone();
                if m.is_empty() {
                    continue;
                }
                let muts = 1 + rng.below(5);
                for _ in 0..muts {
                    let i = rng.below(m.len());
                    m[i] ^= rng.byte();
                }
                // mutation may also truncate
                if rng.below(4) == 0 && !m.is_empty() {
                    let cut = rng.below(m.len());
                    m.truncate(cut);
                }
                let _ = detect(&m); // must not panic
            }
        }
    }

    #[test]
    fn fuzz_filename_hint_never_panic() {
        let mut rng = Rng::new(0xCAFE_BABE_DEAD_BEEF);
        let pool: [&str; 11] = [
            ".", "/", "\\", "\0", "a", ".png", "..", "файл", "🦀", "tar.gz", "Z",
        ];
        let bytes = vec![0x89u8, 0x50, 0x4E, 0x47];
        for _ in 0..20_000 {
            let segs = rng.below(30);
            let mut name = alloc::string::String::new();
            for _ in 0..segs {
                name.push_str(pool[rng.below(pool.len())]);
            }
            let _ = detect_with_hint(&bytes, Some(&name)); // must not panic
            let _ = extract_ext(&name);
        }
        // explicit pathological names
        for name in &["", ".", "..", "...", "/", "\\", "/////", "a/b/c/", ".\0"] {
            let _ = detect_with_hint(b"hello", Some(name));
        }
    }
}
