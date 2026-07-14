//! # RaeXlsx writer — build a minimal **valid** `.xlsx` from a cell model.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy / Distribution criterion #5 — "edit
//! AND SAVE my spreadsheets": a daily driver that can only *open* `.xlsx` is a
//! viewer, not an editor. This module is the inverse of the reader in
//! [`crate`]: given an in-memory [`crate::Workbook`] (sheets of [`crate::Cell`]s)
//! it serializes a minimal, spec-correct Office Open XML / SpreadsheetML package
//! and zips it with [`ath_zip::ZipWriter`] — it does **not** hand-roll ZIP/DEFLATE.
//!
//! ## The verification lever (write → read identity)
//! Every workbook this writer produces is read back by the existing
//! [`crate::Workbook::open`] reader to the *same* model: sheet names + order, each
//! cell's exact typed value (string text, `Number` `f64`, `Bool`), the sparse gaps
//! (omitted cells), and a de-duplicated shared-string table. The host KAT suite
//! (`cargo test -p ath_xlsx`) is the load-bearing proof; tweak any expected value
//! and a round-trip assert flips.
//!
//! ## OOXML parts emitted (the minimal real `.xlsx` shape the reader consumes)
//! - `[Content_Types].xml` — `rels`/`xml` defaults + overrides for the workbook,
//!   each worksheet, and sharedStrings.
//! - `_rels/.rels` — the root relationship → `xl/workbook.xml`.
//! - `xl/workbook.xml` — `<sheets>` with each sheet's `name`, `sheetId`, and `r:id`.
//! - `xl/_rels/workbook.xml.rels` — `rId` → `worksheets/sheetN.xml` + sharedStrings.
//! - `xl/sharedStrings.xml` — the interned string table (string cells de-duplicated
//!   to one `<si>` each; `count` / `uniqueCount` set).
//! - `xl/worksheets/sheetN.xml` — `<sheetData>` of `<row r><c r= t=><v>` cells:
//!   shared strings as `t="s"` + the sst index, numbers as a plain `<v>`, booleans
//!   `t="b"` with `1`/`0`, errors `t="e"`. Empty cells are omitted (sparse).
//!
//! ## Number serialization (round-trip-stable)
//! `core` has no `f64` formatter, and the reader's [`crate::parse_f64`] reconstructs
//! a number from its decimal `<v>` text. [`serialize_number`] emits the *shortest*
//! decimal that the reader's own `parse_f64` reads back to the bit-identical `f64`
//! (an integer `42.0` → `"42"`, a fraction → just enough digits), so write→read is
//! exact for every value the reader can represent. Non-finite values are clamped to
//! a finite literal (Excel cannot store NaN/inf in a `<v>`).
//!
//! ## Bounded / never-corrupt
//! The writer caps the sheet count ([`MAX_WRITE_SHEETS`]), the per-sheet cell count
//! ([`MAX_WRITE_CELLS`]), and a cell's column/row to the same Excel limits the
//! reader enforces ([`crate::MAX_COL`] / [`crate::MAX_ROW`]); over-limit input is an
//! [`XlsxWriteError`]. [`ath_zip::ZipWriter`] bounds the archive itself. XML text is
//! always escaped — the writer never emits malformed XML.

use alloc::string::String;
use alloc::vec::Vec;

use ath_zip::{Method, ZipWriter};

use crate::{Cell, CellValue, Sheet, Workbook, MAX_COL, MAX_ROW};

/// Largest number of worksheets the writer will serialize.
pub const MAX_WRITE_SHEETS: usize = 4096;

/// Largest number of cells the writer will serialize for a single sheet.
pub const MAX_WRITE_CELLS: usize = 4 * 1024 * 1024;

/// An error building an `.xlsx`. Every variant is a handled path — never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XlsxWriteError {
    /// No sheets were added (a workbook must have at least one worksheet).
    NoSheets,
    /// The sheet count exceeded [`MAX_WRITE_SHEETS`].
    TooManySheets,
    /// A sheet's cell count exceeded [`MAX_WRITE_CELLS`].
    TooManyCells,
    /// A cell's column or row index exceeded the Excel limits
    /// ([`crate::MAX_COL`] / [`crate::MAX_ROW`]).
    BadCellRef,
    /// The ZIP layer refused the archive (over a 32-bit ZIP bound, etc.).
    Zip,
}

impl From<ath_zip::ZipError> for XlsxWriteError {
    fn from(_: ath_zip::ZipError) -> Self {
        XlsxWriteError::Zip
    }
}

/// A fluent builder for an `.xlsx` workbook.
///
/// Add sheets with [`WorkbookBuilder::add_sheet`] (a name + its cells), then call
/// [`WorkbookBuilder::to_xlsx`] for the complete `.xlsx` bytes. The result is read
/// back identically by [`crate::Workbook::open`].
#[derive(Default)]
pub struct WorkbookBuilder {
    sheets: Vec<Sheet>,
}

impl WorkbookBuilder {
    /// Create an empty builder.
    pub fn new() -> Self {
        WorkbookBuilder { sheets: Vec::new() }
    }

    /// Add a worksheet by name and its cells (sparse — omit empty cells).
    ///
    /// The writer serializes from [`Sheet::cells`] directly, so the [`Sheet`]'s
    /// private cached-dimension fields are irrelevant here — they are recomputed by
    /// the reader on round-trip.
    pub fn add_sheet(mut self, name: &str, cells: Vec<Cell>) -> Self {
        let sheet = Sheet {
            name: String::from(name),
            cells,
            ..Sheet::default()
        };
        self.sheets.push(sheet);
        self
    }

    /// Add an already-built [`Sheet`].
    pub fn add_sheet_struct(mut self, sheet: Sheet) -> Self {
        self.sheets.push(sheet);
        self
    }

    /// Serialize the builder's sheets to a complete `.xlsx` byte vector.
    ///
    /// Builds the OOXML parts (content-types, rels, workbook, sharedStrings, one
    /// worksheet per sheet) and zips them via [`ath_zip::ZipWriter`]. Returns `Err`
    /// on an empty workbook, an over-limit sheet/cell count, an out-of-bounds cell
    /// reference, or a ZIP-layer refusal. Never produces a corrupt file.
    pub fn to_xlsx(&self) -> Result<Vec<u8>, XlsxWriteError> {
        serialize(&self.sheets)
    }
}

impl Workbook {
    /// Serialize this workbook to a complete `.xlsx` byte vector (see
    /// [`WorkbookBuilder::to_xlsx`]). The inverse of [`Workbook::open`].
    pub fn to_xlsx(&self) -> Result<Vec<u8>, XlsxWriteError> {
        serialize(&self.sheets)
    }
}

// ─── Core serialization ─────────────────────────────────────────────────────

fn serialize(sheets: &[Sheet]) -> Result<Vec<u8>, XlsxWriteError> {
    if sheets.is_empty() {
        return Err(XlsxWriteError::NoSheets);
    }
    if sheets.len() > MAX_WRITE_SHEETS {
        return Err(XlsxWriteError::TooManySheets);
    }

    // --- Build the shared-string table: dedupe every string-valued cell to one
    // entry, recording each cell's resolved sst index in document order.
    let mut sst = SharedStrings::new();
    for sheet in sheets {
        if sheet.cells.len() > MAX_WRITE_CELLS {
            return Err(XlsxWriteError::TooManyCells);
        }
        for c in &sheet.cells {
            if c.col > MAX_COL || c.row >= MAX_ROW {
                return Err(XlsxWriteError::BadCellRef);
            }
            if let CellValue::Text(s) = &c.value {
                sst.intern(s);
            }
        }
    }

    let mut zw = ZipWriter::new();

    // --- [Content_Types].xml
    zw.add_file(
        "[Content_Types].xml",
        content_types_xml(sheets.len()).as_bytes(),
        Method::Deflate,
    )?;

    // --- _rels/.rels (root rel → workbook)
    zw.add_file("_rels/.rels", ROOT_RELS.as_bytes(), Method::Deflate)?;

    // --- xl/workbook.xml
    zw.add_file(
        "xl/workbook.xml",
        workbook_xml(sheets).as_bytes(),
        Method::Deflate,
    )?;

    // --- xl/_rels/workbook.xml.rels
    zw.add_file(
        "xl/_rels/workbook.xml.rels",
        workbook_rels_xml(sheets.len()).as_bytes(),
        Method::Deflate,
    )?;

    // --- xl/sharedStrings.xml
    zw.add_file(
        "xl/sharedStrings.xml",
        sst.to_xml().as_bytes(),
        Method::Deflate,
    )?;

    // --- xl/worksheets/sheetN.xml
    for (i, sheet) in sheets.iter().enumerate() {
        let mut path = String::from("xl/worksheets/sheet");
        push_u32(&mut path, (i as u32) + 1);
        path.push_str(".xml");
        let body = worksheet_xml(sheet, &sst);
        zw.add_file(&path, body.as_bytes(), Method::Deflate)?;
    }

    Ok(zw.finish()?)
}

// ─── Shared strings (dedup) ─────────────────────────────────────────────────

/// The interned shared-string table: a list of unique strings plus a lookup from
/// string → index. `intern` is idempotent so a duplicate string maps to one entry.
struct SharedStrings {
    list: Vec<String>,
}

impl SharedStrings {
    fn new() -> Self {
        SharedStrings { list: Vec::new() }
    }

    /// Intern a string, returning its sst index. A repeat of an already-present
    /// string returns the existing index (the dedup property the KAT proves).
    fn intern(&mut self, s: &str) -> usize {
        if let Some(i) = self.list.iter().position(|e| e == s) {
            return i;
        }
        self.list.push(String::from(s));
        self.list.len() - 1
    }

    /// The index of an already-interned string (every string cell was interned in
    /// the pre-pass), or `0` as a safe fallback that never indexes out of bounds.
    fn index_of(&self, s: &str) -> usize {
        self.list.iter().position(|e| e == s).unwrap_or(0)
    }

    /// Serialize the `<sst>` part with `count` (total references — we emit one per
    /// unique entry, which is a valid lower bound Excel tolerates) and
    /// `uniqueCount` (the number of `<si>` entries).
    fn to_xml(&self) -> String {
        let unique = self.list.len();
        let mut s = String::new();
        s.push_str(XML_DECL);
        s.push_str(
            "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" count=\"",
        );
        push_u32(&mut s, unique as u32);
        s.push_str("\" uniqueCount=\"");
        push_u32(&mut s, unique as u32);
        s.push_str("\">");
        for entry in &self.list {
            // `xml:space="preserve"` so leading/trailing whitespace round-trips
            // (the reader trims a <t> without it).
            s.push_str("<si><t xml:space=\"preserve\">");
            push_escaped(&mut s, entry);
            s.push_str("</t></si>");
        }
        s.push_str("</sst>");
        s
    }
}

// ─── Part builders ──────────────────────────────────────────────────────────

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/>\
</Relationships>";

/// `[Content_Types].xml`: defaults for `rels`/`xml`, plus overrides for the
/// workbook, sharedStrings, and each worksheet part.
fn content_types_xml(sheet_count: usize) -> String {
    let mut s = String::new();
    s.push_str(XML_DECL);
    s.push_str("<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">");
    s.push_str("<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>");
    s.push_str("<Default Extension=\"xml\" ContentType=\"application/xml\"/>");
    s.push_str("<Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>");
    s.push_str("<Override PartName=\"/xl/sharedStrings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml\"/>");
    for i in 0..sheet_count {
        s.push_str("<Override PartName=\"/xl/worksheets/sheet");
        push_u32(&mut s, (i as u32) + 1);
        s.push_str(".xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>");
    }
    s.push_str("</Types>");
    s
}

/// `xl/workbook.xml`: the `<sheets>` list. Each sheet gets `sheetId = i+1` and
/// `r:id = rId(i+1)` (the rels below map `rId(i+1)` → `worksheets/sheet(i+1).xml`).
fn workbook_xml(sheets: &[Sheet]) -> String {
    let mut s = String::new();
    s.push_str(XML_DECL);
    s.push_str("<workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>");
    for (i, sheet) in sheets.iter().enumerate() {
        let n = (i as u32) + 1;
        s.push_str("<sheet name=\"");
        push_escaped(&mut s, &sheet.name);
        s.push_str("\" sheetId=\"");
        push_u32(&mut s, n);
        s.push_str("\" r:id=\"rId");
        push_u32(&mut s, n);
        s.push_str("\"/>");
    }
    s.push_str("</sheets></workbook>");
    s
}

/// `xl/_rels/workbook.xml.rels`: `rId(i+1)` → `worksheets/sheet(i+1).xml` for each
/// sheet, plus a trailing rel for the shared-string table.
fn workbook_rels_xml(sheet_count: usize) -> String {
    const WS_TYPE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
    const SST_TYPE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";
    let mut s = String::new();
    s.push_str(XML_DECL);
    s.push_str(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for i in 0..sheet_count {
        let n = (i as u32) + 1;
        s.push_str("<Relationship Id=\"rId");
        push_u32(&mut s, n);
        s.push_str("\" Type=\"");
        s.push_str(WS_TYPE);
        s.push_str("\" Target=\"worksheets/sheet");
        push_u32(&mut s, n);
        s.push_str(".xml\"/>");
    }
    // The sharedStrings rel takes the id after the last worksheet.
    s.push_str("<Relationship Id=\"rId");
    push_u32(&mut s, (sheet_count as u32) + 1);
    s.push_str("\" Type=\"");
    s.push_str(SST_TYPE);
    s.push_str("\" Target=\"sharedStrings.xml\"/>");
    s.push_str("</Relationships>");
    s
}

/// `xl/worksheets/sheetN.xml`: `<sheetData>` of rows. Cells are grouped by row in
/// ascending `(row, col)` order; empty cells are omitted (sparse). A correct A1
/// reference is emitted for every cell.
fn worksheet_xml(sheet: &Sheet, sst: &SharedStrings) -> String {
    // Order cells by (row, col) so rows are contiguous and ascending — the shape
    // Excel writes and the reader expects.
    let mut order: Vec<usize> = (0..sheet.cells.len()).collect();
    order.sort_by(|&a, &b| {
        let ca = &sheet.cells[a];
        let cb = &sheet.cells[b];
        (ca.row, ca.col).cmp(&(cb.row, cb.col))
    });

    let mut s = String::new();
    s.push_str(XML_DECL);
    s.push_str("<worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheetData>");

    let mut idx = 0usize;
    while idx < order.len() {
        let row = sheet.cells[order[idx]].row;
        // Open the row.
        s.push_str("<row r=\"");
        push_u32(&mut s, row + 1);
        s.push_str("\">");
        // Emit every cell sharing this row.
        while idx < order.len() {
            let c = &sheet.cells[order[idx]];
            if c.row != row {
                break;
            }
            emit_cell(&mut s, c, sst);
            idx += 1;
        }
        s.push_str("</row>");
    }

    s.push_str("</sheetData></worksheet>");
    s
}

/// Emit one `<c>` cell. Empty cells are skipped entirely (sparse — the reader
/// returns `None` for an omitted position, preserving the gap).
fn emit_cell(out: &mut String, c: &Cell, sst: &SharedStrings) {
    match &c.value {
        CellValue::Empty => {
            // Omit empty cells so the grid stays sparse (a gap reads back as None).
        }
        CellValue::Text(text) => {
            let i = sst.index_of(text);
            out.push_str("<c r=\"");
            push_a1(out, c.col, c.row);
            out.push_str("\" t=\"s\"><v>");
            push_u32(out, i as u32);
            out.push_str("</v></c>");
        }
        CellValue::Number(n) => {
            out.push_str("<c r=\"");
            push_a1(out, c.col, c.row);
            out.push_str("\"><v>");
            out.push_str(&serialize_number(*n));
            out.push_str("</v></c>");
        }
        CellValue::Bool(b) => {
            out.push_str("<c r=\"");
            push_a1(out, c.col, c.row);
            out.push_str("\" t=\"b\"><v>");
            out.push_str(if *b { "1" } else { "0" });
            out.push_str("</v></c>");
        }
        CellValue::Error(e) => {
            out.push_str("<c r=\"");
            push_a1(out, c.col, c.row);
            out.push_str("\" t=\"e\"><v>");
            push_escaped(out, e);
            out.push_str("</v></c>");
        }
    }
}

// ─── Number serialization (round-trips through the reader's parse_f64) ────────

/// Serialize an `f64` to the shortest decimal string the reader's
/// [`crate::parse_f64`] reads back to the bit-identical value.
///
/// Strategy: an integral value within `u64` range prints with no fractional part
/// (so `42.0` → `"42"`, which the reader parses back to `Number(42.0)`). Otherwise
/// try increasing fractional precision (1..=17 digits) and return the first whose
/// `parse_f64` round-trips exactly; 17 significant digits always suffice to
/// uniquely identify an `f64`. Non-finite inputs (Excel cannot store them) clamp to
/// a finite literal so output is never malformed.
pub fn serialize_number(n: f64) -> String {
    if n.is_nan() {
        return String::from("0");
    }
    if n.is_infinite() {
        // Largest/smallest finite-ish literal the reader will accept (no inf in <v>).
        return String::from(if n < 0.0 { "-1e308" } else { "1e308" });
    }
    if n == 0.0 {
        // Covers +0.0 and -0.0; the reader parses "0" to 0.0.
        return String::from("0");
    }

    // --- Exact integer fast path (covers the common "42" case bit-exactly).
    let abs = if n < 0.0 { -n } else { n };
    if abs < 1.8e19 && (abs as u64 as f64) == abs {
        let mut s = String::new();
        let mut v = n;
        if v < 0.0 {
            s.push('-');
            v = -v;
        }
        push_u64(&mut s, v as u64);
        // Confirm the integer form round-trips before trusting it.
        if let Some(back) = crate::parse_f64(&s) {
            if back == n {
                return s;
            }
        }
    }

    // --- General path: shortest fractional representation that round-trips.
    for prec in 1..=17u32 {
        let s = format_fixed(n, prec);
        if let Some(back) = crate::parse_f64(&s) {
            if back == n {
                return s;
            }
        }
        // Also try scientific form for very large/small magnitudes where fixed
        // notation would need an impractical digit count.
        let sci = format_scientific(n, prec);
        if let Some(back) = crate::parse_f64(&sci) {
            if back == n {
                return sci;
            }
        }
    }

    // Fallback: the maximum-precision fixed form (always round-trips for finite f64
    // within the reader's range; the loop above will normally have returned first).
    format_fixed(n, 17)
}

/// Format `n` in fixed-point notation with `frac` fractional digits, trailing zeros
/// trimmed. No `format!`/float formatter (no_std-safe). The mantissa is built by
/// scaling so the digits are exact for the precision requested.
fn format_fixed(n: f64, frac: u32) -> String {
    let mut out = String::new();
    let mut x = n;
    if x < 0.0 {
        out.push('-');
        x = -x;
    }
    let int_part = trunc_u128(x);
    push_u128(&mut out, int_part);

    if frac > 0 {
        // Fractional digits, one at a time, from the scaled remainder.
        let mut rem = x - (int_part as f64);
        let mut digits = String::new();
        let mut k = frac;
        while k > 0 {
            rem *= 10.0;
            let d = trunc_u128(rem) as u8;
            let d = if d > 9 { 9 } else { d };
            digits.push((b'0' + d) as char);
            rem -= d as f64;
            k -= 1;
        }
        while digits.ends_with('0') {
            digits.pop();
        }
        if !digits.is_empty() {
            out.push('.');
            out.push_str(&digits);
        }
    }
    out
}

/// Format `n` as `D.DDDDeEE` scientific notation with `sig-1` fractional digits in
/// the mantissa. Used for magnitudes where fixed notation is impractical.
fn format_scientific(n: f64, sig: u32) -> String {
    let mut out = String::new();
    let mut x = n;
    if x < 0.0 {
        out.push('-');
        x = -x;
    }
    if x == 0.0 {
        out.push('0');
        return out;
    }
    // Find the base-10 exponent e such that 1 <= x / 10^e < 10.
    let mut e: i32 = 0;
    while x >= 10.0 {
        x /= 10.0;
        e += 1;
        if e > 308 {
            break;
        }
    }
    while x < 1.0 {
        x *= 10.0;
        e -= 1;
        if e < -323 {
            break;
        }
    }
    // Mantissa: one integer digit + (sig-1) fractional.
    let lead = trunc_u128(x) as u8;
    out.push((b'0' + if lead > 9 { 9 } else { lead }) as char);
    if sig > 1 {
        let mut rem = x - (lead as f64);
        let mut digits = String::new();
        let mut k = sig - 1;
        while k > 0 {
            rem *= 10.0;
            let d = trunc_u128(rem) as u8;
            let d = if d > 9 { 9 } else { d };
            digits.push((b'0' + d) as char);
            rem -= d as f64;
            k -= 1;
        }
        while digits.ends_with('0') {
            digits.pop();
        }
        if !digits.is_empty() {
            out.push('.');
            out.push_str(&digits);
        }
    }
    out.push('e');
    if e < 0 {
        out.push('-');
        push_u32(&mut out, (-e) as u32);
    } else {
        push_u32(&mut out, e as u32);
    }
    out
}

/// Truncate a non-negative finite `f64` toward zero into a `u128` (exact for the
/// magnitudes spreadsheets carry; clamps at the `u128` ceiling).
fn trunc_u128(x: f64) -> u128 {
    if x <= 0.0 {
        return 0;
    }
    if x >= 3.402_823_669_209_385e38 {
        return u128::MAX;
    }
    x as u128
}

// ─── XML escaping + small numeric/A1 emitters (no_std-safe) ──────────────────

/// Append `s` to `out`, escaping the five XML metacharacters and control chars
/// (other than tab/CR/LF, which XML 1.0 permits in text). Never emits a byte that
/// would make the part malformed.
fn push_escaped(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {
                // Disallowed XML 1.0 control char: emit a numeric char reference so
                // the document stays well-formed (the reader decodes it back).
                out.push_str("&#");
                push_u32(out, c as u32);
                out.push(';');
            }
            c => out.push(c),
        }
    }
}

/// Append an A1 cell reference (`col`,`row` both 0-based) to `out`.
fn push_a1(out: &mut String, col: u32, row: u32) {
    // Column letters: bijective base-26 (0 → A, 25 → Z, 26 → AA).
    let mut n = col + 1;
    let mut buf = [0u8; 8];
    let mut i = buf.len();
    while n > 0 {
        let rem = ((n - 1) % 26) as u8;
        i -= 1;
        buf[i] = b'A' + rem;
        n = (n - 1) / 26;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
    push_u32(out, row + 1);
}

/// Append a `u32` in decimal.
fn push_u32(out: &mut String, v: u32) {
    push_u128(out, v as u128);
}

/// Append a `u64` in decimal.
fn push_u64(out: &mut String, v: u64) {
    push_u128(out, v as u128);
}

/// Append a `u128` in decimal (the single decimal emitter the others delegate to).
fn push_u128(out: &mut String, mut v: u128) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 40];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        out.push(b as char);
    }
}
