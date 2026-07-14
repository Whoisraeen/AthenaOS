//! PDF document generator — turns a print job's text + page setup into a valid
//! PDF 1.7 byte stream (the format every printer driver and "Save as PDF" path
//! consumes).
//!
//! LEGACY_GAMING_CONCEPT.md "built for people who care about how things feel": a daily
//! driver has to be able to *produce printable output* — Ctrl+P → a real file a
//! printer (or another machine) can render. PostScript generation already lives
//! in `FilterChain::text_to_postscript`; PDF is the universal modern target, so
//! this module emits a minimal-but-spec-valid PDF: the `%PDF-1.7` header, a
//! cross-reference-addressed object graph (Catalog → Pages → Page → Contents +
//! a base-14 Helvetica/Courier font), one content stream per page laid out from
//! the [`crate::PageLayout`] geometry, then a correct `xref` table and trailer.
//!
//! Content streams are emitted uncompressed by default; [`PdfBuilder::compressed`]
//! wraps each stream in a zlib (`FlateDecode`) filter using the from-scratch
//! `ath_deflate` codec — the same property `inflate(deflate(x)) == x` makes the
//! output round-trippable by any conformant reader.
//!
//! Every offset in the `xref` table is computed from the actual emitted byte
//! length, so the table is self-consistent by construction; the host KATs assert
//! the `%PDF` magic, the object count, the page count, a parseable `startxref`
//! pointing at the `xref` keyword, and that requested text appears in a content
//! stream. Malformed input (e.g. a request for zero pages) yields a clean `Err`,
//! never a panic.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{MediaSize, Orientation, PageLayout};

/// Errors from PDF generation. Every variant is a handled path — generation
/// never panics on caller input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfError {
    /// A document with no pages was requested.
    NoPages,
    /// A page's media dimensions were degenerate (zero width or height).
    DegenerateMedia,
    /// More objects than the generator supports in one document.
    TooManyObjects,
}

/// One of the PDF base-14 fonts we embed by reference (no font program needed —
/// every conformant reader ships these). Courier is fixed-width (good for
/// terminal/code dumps); Helvetica is the default proportional face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseFont {
    Helvetica,
    Courier,
    TimesRoman,
}

impl BaseFont {
    fn base_font_name(self) -> &'static str {
        match self {
            BaseFont::Helvetica => "Helvetica",
            BaseFont::Courier => "Courier",
            BaseFont::TimesRoman => "Times-Roman",
        }
    }
}

/// One logical page of text to render into the PDF.
#[derive(Debug, Clone)]
pub struct PdfPage {
    /// Page media + margins + orientation; drives the MediaBox and text origin.
    pub layout: PageLayout,
    /// The lines of text to lay out top-to-bottom. Already split on newlines.
    pub lines: Vec<String>,
    /// Point size of the text.
    pub font_size: u32,
    /// Baseline-to-baseline leading in points.
    pub leading: u32,
    /// Which base-14 font to use.
    pub font: BaseFont,
}

impl PdfPage {
    pub fn new(layout: PageLayout) -> Self {
        Self {
            layout,
            lines: Vec::new(),
            font_size: 12,
            leading: 14,
            font: BaseFont::Helvetica,
        }
    }

    /// Split raw UTF-8 text on newlines into lines for this page (no automatic
    /// wrapping — callers that need wrapping pre-split; this is the deterministic
    /// primitive the KATs assert against).
    pub fn from_text(layout: PageLayout, text: &str, font: BaseFont) -> Self {
        let mut page = Self::new(layout);
        page.font = font;
        for line in text.split('\n') {
            page.lines.push(String::from(line));
        }
        page
    }

    /// The MediaBox dimensions in PDF user-space points, accounting for
    /// landscape orientation (which swaps width/height).
    fn media_box(&self) -> (u32, u32) {
        let w = self.layout.media.width_points();
        let h = self.layout.media.height_points();
        match self.layout.orientation {
            Orientation::Landscape | Orientation::ReverseLandscape => (h, w),
            _ => (w, h),
        }
    }
}

/// Builds a complete PDF document from a sequence of pages.
pub struct PdfBuilder {
    pages: Vec<PdfPage>,
    title: String,
    compress: bool,
}

impl PdfBuilder {
    pub fn new() -> Self {
        Self {
            pages: Vec::new(),
            title: String::from("RaePrint Document"),
            compress: false,
        }
    }

    /// Builder that FlateDecode-compresses every content stream.
    pub fn compressed() -> Self {
        let mut b = Self::new();
        b.compress = true;
        b
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = String::from(title);
    }

    pub fn add_page(&mut self, page: PdfPage) {
        self.pages.push(page);
    }

    /// Convenience: build a single-page document from one block of text under a
    /// default Letter portrait layout.
    pub fn single_text_page(text: &str, font: BaseFont) -> Self {
        let mut b = Self::new();
        let layout = PageLayout::new(MediaSize::Letter);
        b.add_page(PdfPage::from_text(layout, text, font));
        b
    }

    /// PDF object count for `self.pages` pages:
    ///   1 Catalog + 1 Pages tree + 1 shared Font + 2 per page (Page + Contents).
    fn object_count(&self) -> usize {
        3 + self.pages.len() * 2
    }

    /// Emit the full PDF as bytes. Returns `Err` for an empty document or
    /// degenerate page geometry — never panics.
    pub fn build(&self) -> Result<Vec<u8>, PdfError> {
        if self.pages.is_empty() {
            return Err(PdfError::NoPages);
        }
        for p in &self.pages {
            let (w, h) = p.media_box();
            if w == 0 || h == 0 {
                return Err(PdfError::DegenerateMedia);
            }
        }
        let total_objects = self.object_count();
        if total_objects > 8192 {
            return Err(PdfError::TooManyObjects);
        }

        // Object numbering plan (1-based):
        //   1: Catalog
        //   2: Pages tree
        //   3: Font (shared)
        //   4 + 2*i:     Page i
        //   4 + 2*i + 1: Contents i
        let font_obj = 3usize;
        let first_page_obj = 4usize;

        // We accumulate body bytes and record each object's byte offset (from the
        // very start of the file) so the xref table is exact.
        let mut out: Vec<u8> = Vec::new();
        // xref offsets indexed by object number (index 0 = the free object).
        let mut offsets: Vec<usize> = Vec::with_capacity(total_objects + 1);
        offsets.push(0); // object 0 is the head of the free list.
        for _ in 0..total_objects {
            offsets.push(0);
        }

        out.extend_from_slice(b"%PDF-1.7\n");
        // Binary comment marker (recommended so tools treat the file as binary).
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

        // --- Object 1: Catalog ---
        offsets[1] = out.len();
        out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

        // --- Object 2: Pages tree ---
        offsets[2] = out.len();
        let mut kids = String::new();
        for i in 0..self.pages.len() {
            let page_obj = first_page_obj + i * 2;
            kids.push_str(&format!("{} 0 R ", page_obj));
        }
        let pages_obj = format!(
            "2 0 obj\n<< /Type /Pages /Count {} /Kids [ {}] >>\nendobj\n",
            self.pages.len(),
            kids
        );
        out.extend_from_slice(pages_obj.as_bytes());

        // --- Object 3: shared Font (the first page's font wins as the shared
        // resource; per-page fonts would be a follow-up, but every page
        // references /F1, so a mismatched face simply renders in the shared one). ---
        offsets[font_obj] = out.len();
        let font_name = self.pages[0].font.base_font_name();
        let font_dict = format!(
            "3 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /{} >>\nendobj\n",
            font_name
        );
        out.extend_from_slice(font_dict.as_bytes());

        // --- Per-page Page + Contents objects ---
        for (i, page) in self.pages.iter().enumerate() {
            let page_obj = first_page_obj + i * 2;
            let contents_obj = page_obj + 1;
            let (mb_w, mb_h) = page.media_box();

            // Build the content stream first so we know its length.
            let raw_stream = build_content_stream(page, mb_h);
            let (stream_bytes, filter) = if self.compress {
                (
                    ath_deflate::zlib_compress(&raw_stream),
                    " /Filter /FlateDecode",
                )
            } else {
                (raw_stream, "")
            };

            // Page object.
            offsets[page_obj] = out.len();
            let page_dict = format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [ 0 0 {} {} ] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>\nendobj\n",
                page_obj, mb_w, mb_h, contents_obj
            );
            out.extend_from_slice(page_dict.as_bytes());

            // Contents object.
            offsets[contents_obj] = out.len();
            let head = format!(
                "{} 0 obj\n<< /Length {}{} >>\nstream\n",
                contents_obj,
                stream_bytes.len(),
                filter
            );
            out.extend_from_slice(head.as_bytes());
            out.extend_from_slice(&stream_bytes);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        }

        // --- xref table ---
        let xref_offset = out.len();
        let xref_header = format!("xref\n0 {}\n", total_objects + 1);
        out.extend_from_slice(xref_header.as_bytes());
        // Object 0: the head of the free list — exactly "0000000000 65535 f \n".
        out.extend_from_slice(b"0000000000 65535 f \n");
        for obj in 1..=total_objects {
            // 10-digit zero-padded offset, generation 00000, in-use 'n'.
            let entry = format!("{:010} {:05} n \n", offsets[obj], 0);
            out.extend_from_slice(entry.as_bytes());
        }

        // --- trailer ---
        let trailer = format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            total_objects + 1,
            xref_offset
        );
        out.extend_from_slice(trailer.as_bytes());

        Ok(out)
    }
}

/// Build the raw (pre-compression) content stream for one page: set the font,
/// begin a text object at the top-left text origin, and emit each line with the
/// leading applied between baselines. `media_h` is the (orientation-adjusted)
/// page height used to flip from top-origin layout to PDF's bottom-origin space.
fn build_content_stream(page: &PdfPage, media_h: u32) -> Vec<u8> {
    let mut s: Vec<u8> = Vec::new();
    let size = page.font_size.max(1);
    let leading = page.leading.max(1);

    // Text origin: left margin in, top margin down from the top edge.
    let x = page.layout.margins.left;
    let top_y = media_h.saturating_sub(page.layout.margins.top + size);

    s.extend_from_slice(b"BT\n");
    s.extend_from_slice(format!("/F1 {} Tf\n", size).as_bytes());
    s.extend_from_slice(format!("{} TL\n", leading).as_bytes());
    s.extend_from_slice(format!("{} {} Td\n", x, top_y).as_bytes());

    let mut first = true;
    for line in &page.lines {
        if !first {
            // T* advances to the next line by the set leading.
            s.extend_from_slice(b"T*\n");
        }
        first = false;
        s.push(b'(');
        for &ch in line.as_bytes() {
            match ch {
                b'(' => s.extend_from_slice(b"\\("),
                b')' => s.extend_from_slice(b"\\)"),
                b'\\' => s.extend_from_slice(b"\\\\"),
                c if (0x20..0x7F).contains(&c) => s.push(c),
                _ => s.push(b'?'),
            }
        }
        s.extend_from_slice(b") Tj\n");
    }
    s.extend_from_slice(b"ET\n");
    s
}

// ===========================================================================
// Page-setup geometry — fit-to-page / scaling math (pure, host-KAT-able)
// ===========================================================================

/// The result of fitting a source content box into a printable area: the scale
/// factor (as a fraction, scaled by 1000 to stay integer/no-float in callers)
/// and the centered origin offset within the printable area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FitResult {
    /// Scale factor expressed in per-mille (1000 == 100%, 500 == 50%).
    pub scale_permille: u32,
    /// Scaled content width in points.
    pub scaled_w: u32,
    /// Scaled content height in points.
    pub scaled_h: u32,
    /// X offset to center the scaled content in the printable area.
    pub offset_x: u32,
    /// Y offset to center the scaled content in the printable area.
    pub offset_y: u32,
}

/// Compute fit-to-page scaling: scale a `src_w x src_h` content box to fit
/// inside a `avail_w x avail_h` printable area, preserving aspect ratio, never
/// upscaling past 100% (that is "shrink-to-fit"/"fit-to-page" semantics), and
/// center the result. Returns `None` for a degenerate (zero) source/area.
pub fn fit_to_page(src_w: u32, src_h: u32, avail_w: u32, avail_h: u32) -> Option<FitResult> {
    if src_w == 0 || src_h == 0 || avail_w == 0 || avail_h == 0 {
        return None;
    }
    // scale = min(avail_w/src_w, avail_h/src_h), capped at 1.0, in per-mille.
    let sx = (avail_w as u64 * 1000) / src_w as u64;
    let sy = (avail_h as u64 * 1000) / src_h as u64;
    let mut scale = sx.min(sy);
    if scale > 1000 {
        scale = 1000; // never upscale for fit-to-page.
    }
    let scaled_w = ((src_w as u64 * scale) / 1000) as u32;
    let scaled_h = ((src_h as u64 * scale) / 1000) as u32;
    let offset_x = (avail_w.saturating_sub(scaled_w)) / 2;
    let offset_y = (avail_h.saturating_sub(scaled_h)) / 2;
    Some(FitResult {
        scale_permille: scale as u32,
        scaled_w,
        scaled_h,
        offset_x,
        offset_y,
    })
}

/// Compute the per-cell rectangle geometry for an N-up layout: divide the
/// printable area into `cols x rows` cells. Returns `(cell_w, cell_h)` in points,
/// or `None` for a zero grid / zero area.
pub fn n_up_cell(avail_w: u32, avail_h: u32, cols: u32, rows: u32) -> Option<(u32, u32)> {
    if cols == 0 || rows == 0 || avail_w == 0 || avail_h == 0 {
        return None;
    }
    Some((avail_w / cols, avail_h / rows))
}

/// Map an `n_up` count to a (cols, rows) grid the way print dialogs do
/// (1,2,4,6,9,16 are the common presets). Falls back to a roughly-square grid.
pub fn n_up_grid(n_up: u32) -> (u32, u32) {
    match n_up {
        0 | 1 => (1, 1),
        2 => (1, 2),
        4 => (2, 2),
        6 => (2, 3),
        9 => (3, 3),
        16 => (4, 4),
        n => {
            // ceil(sqrt(n)) columns, enough rows to hold n.
            let mut cols = 1u32;
            while cols * cols < n {
                cols += 1;
            }
            let rows = (n + cols - 1) / cols;
            (cols, rows)
        }
    }
}
