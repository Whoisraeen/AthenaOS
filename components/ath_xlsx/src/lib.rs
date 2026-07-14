//! # RaeXlsx — a never-panic, `no_std` XLSX (Excel) reader (Office Open XML / SpreadsheetML).
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy ("how to actually win" — let people
//! switch without conscious effort): a Windows/macOS switcher arrives with a folder
//! of `.xlsx` files, and "open my spreadsheets" is core office-style productivity
//! table stakes for a daily driver (the natural follow-up to `ath_docx`'s "open my
//! Word documents"). This crate is the from-scratch reader the Files Quick Look
//! preview, a future viewer/editor, and the "just show me the data" path sit on.
//!
//! ## What an `.xlsx` is
//! An `.xlsx` is a **ZIP archive** (Open Packaging Conventions) whose members are
//! Office Open XML parts. The cells live in `xl/worksheets/sheetN.xml` as
//! **SpreadsheetML**; most text is interned in `xl/sharedStrings.xml` and referenced
//! by index; the sheet names + order come from `xl/workbook.xml` resolved through
//! `xl/_rels/workbook.xml.rels`. This crate REUSES [`ath_zip`] for the archive layer
//! — it does **not** reimplement ZIP/DEFLATE — and adds a minimal hand-rolled XML
//! parser over those parts (which are well-formed, machine-generated XML; a general
//! XML engine is unnecessary).
//!
//! ## What it models
//! - The package: opens via [`ath_zip`], sanity-checks `[Content_Types].xml`,
//!   reads `xl/workbook.xml` (sheet `name` + `r:id`), `xl/_rels/workbook.xml.rels`
//!   (`rId` → worksheet target), `xl/sharedStrings.xml` (the shared-string table),
//!   and each referenced `xl/worksheets/sheetN.xml`. A ZIP without `xl/workbook.xml`
//!   is rejected as [`XlsxError::NotXlsx`]; at least one worksheet is required.
//! - Shared strings: `<si><t>` and rich-text runs `<si><r><t>…</t></r>…` concatenated.
//! - Cells: the A1 reference on `<c r="B3">` is decoded to a 0-based (col,row) so
//!   sparse rows (omitted empty cells) still land in the right grid position. Cell
//!   types: shared-string (`t="s"` → index into shared strings), inline-string
//!   (`t="inlineStr"` → `<is><t>`), number (default / `t="n"`), boolean
//!   (`t="b"` → TRUE/FALSE), error (`t="e"`), and **formula** cells (`<f>` present) —
//!   for a *reader* we capture the cached `<v>` Excel last computed (and the formula
//!   text) and do **not** evaluate formulas (see Deferred, below).
//! - XML entities (`&amp;` `&lt;` `&gt;` `&quot;` `&apos;`) and numeric character
//!   references (`&#NN;` / `&#xHH;`), plus `xml:space="preserve"`.
//!
//! Output is a structured [`Workbook`] (sheets → cells) for a future viewer/editor,
//! plus [`Sheet::to_csv`] / [`Cell::to_display_string`] for the plain-data path.
//!
//! ## Deferred — honestly
//! - **Formula evaluation.** A reader never recomputes a spreadsheet; that is an
//!   engine (operator precedence, 400+ functions, dependency graph) far beyond
//!   "open my files". We surface the cached value Excel stored ([`CellValue`]) and
//!   the formula text ([`Cell::formula`]); evaluation is explicitly out of scope.
//! - **Date/number formatting.** Excel stores dates as serial day-numbers with a
//!   format applied via `styles.xml`/`numFmt`. We keep the raw [`CellValue::Number`]
//!   and do not interpret a number as a date — formatting belongs to a presentation
//!   layer, not the reader.
//!
//! ## Hostile-input posture (CLAUDE: document parsers are an RCE surface)
//! Every byte handed to [`Workbook::open`] is treated as attacker-controlled. There
//! is **no `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from this
//! crate: a malformed ZIP, a missing `workbook.xml`, an oversized decompressed part,
//! unbalanced/over-nested XML, an absurd cell reference, or a hostile entity are all
//! returned as `Err(XlsxError)`. Amplification vectors are bounded *before/while*
//! building, mirroring ath_docx / ath_png:
//!   - each decompressed XML part is capped at [`MAX_PART_XML`] (ath_zip also
//!     enforces its own per-entry zip-bomb bound first);
//!   - XML element nesting recursion is depth-capped at [`MAX_DEPTH`];
//!   - the total element count per part is capped at [`MAX_ELEMENTS`];
//!   - a cell's resolved column/row is rejected past [`MAX_COL`] / [`MAX_ROW`], and
//!     the per-sheet cell count at [`MAX_CELLS`], so a crafted reference can't
//!     over-allocate a grid; and
//!   - the shared-string count is capped at [`MAX_SHARED_STRINGS`].
//!
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p ath_xlsx`): it hand-assembles a minimal valid `.xlsx` in-test and
//! asserts shared/inline-string text, an exact number `f64`, a boolean, sparse-row
//! placement, multi-sheet names + order, a cached-formula value, the exact
//! [`Sheet::to_csv`] string, and a hostile battery (non-XLSX, truncated, malformed/
//! deep XML, huge cell counts, a seeded fuzz loop) that must all return `Err`/stay
//! bounded with zero panics.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ath_zip::{Archive, ZipError};

// ─── Limits (resource-exhaustion guards, applied before/while building) ───────

/// Largest decompressed size we will parse for any single XML part (workbook,
/// rels, sharedStrings, a worksheet): 64 MiB. A part larger than this is rejected
/// ([`XlsxError::TooLarge`]) rather than parsed. (ath_zip's own
/// [`ath_zip::MAX_ENTRY_SIZE`] / [`ath_zip::MAX_RATIO`] guards fire first on a
/// zip-bomb-shaped entry, before a byte is decompressed.)
pub const MAX_PART_XML: u64 = 64 * 1024 * 1024;

/// Largest XML element nesting depth we will descend. SpreadsheetML nests only a
/// few levels (worksheet → sheetData → row → c → v / is → t); a crafted stream of
/// millions of open tags cannot blow the stack — the parser returns
/// [`XlsxError::TooDeep`] past this bound.
pub const MAX_DEPTH: usize = 256;

/// Largest number of XML start-tags we will process for one part.
pub const MAX_ELEMENTS: usize = 8 * 1024 * 1024;

/// Largest 0-based column index we accept in a cell reference (Excel's own limit is
/// 16383 = column XFD); anything past this is a malformed/hostile reference.
pub const MAX_COL: u32 = 16_383;

/// Largest 1-based row number we accept (Excel's own limit is 1048576).
pub const MAX_ROW: u32 = 1_048_576;

/// Largest number of cells we will retain for a single sheet (defense against a
/// crafted sheet that lists an enormous number of cells).
pub const MAX_CELLS: usize = 4 * 1024 * 1024;

/// Largest number of shared strings we will intern.
pub const MAX_SHARED_STRINGS: usize = 4 * 1024 * 1024;

/// Largest number of worksheets in a workbook.
pub const MAX_SHEETS: usize = 4096;

/// Largest number of grid positions [`Sheet::to_csv`] will materialize: 8 Mi cells.
/// The CSV/full-grid path renders the *populated bounding box* (min..=max of the
/// columns/rows that actually carry cells), not the full `MAX_COL × MAX_ROW`
/// rectangle a single extreme cell would imply. Even so, a legitimately huge but
/// sparse bounding box (e.g. cells at `A1` and `XFD1048576` → a 16384 × 1048576
/// rectangle = ~17.18 billion positions) is refused with [`XlsxError::GridTooLarge`]
/// rather than hung on. 8 Mi positions comfortably covers any real preview while
/// capping worst-case work at a few million emitted fields.
pub const MAX_CSV_CELLS: u64 = 8 * 1024 * 1024;

// ─── Errors ───────────────────────────────────────────────────────────────────

/// An XLSX read error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XlsxError {
    /// The bytes are not a valid ZIP archive (the OPC container is malformed).
    BadZip,
    /// The archive is a valid ZIP but has no `xl/workbook.xml`, or declares no
    /// worksheet — it is not a usable XLSX.
    NotXlsx,
    /// An entry could not be decompressed (corrupt/unsupported per [`ath_zip`]).
    ZipRead,
    /// A decompressed part, or an element/cell/string count, exceeded a bound
    /// ([`MAX_PART_XML`] / [`MAX_ELEMENTS`] / [`MAX_CELLS`] / [`MAX_SHARED_STRINGS`]).
    TooLarge,
    /// XML nesting exceeded [`MAX_DEPTH`].
    TooDeep,
    /// The XML was malformed (unbalanced/unterminated markup, or not valid UTF-8).
    BadXml,
    /// A cell reference (`r="…"`) was malformed or out of the [`MAX_COL`]/[`MAX_ROW`]
    /// bounds.
    BadCellRef,
    /// A full-grid materialization (e.g. [`Sheet::to_csv`]) would emit more than
    /// [`MAX_CSV_CELLS`] grid positions. The sheet's *populated bounding box* is too
    /// large to render eagerly (typically a sparse sheet with cells at extreme
    /// corners, e.g. `A1` + `XFD1048576`), so the materialization is refused rather
    /// than producing a multi-billion-cell hang.
    GridTooLarge,
}

impl From<ZipError> for XlsxError {
    fn from(e: ZipError) -> Self {
        match e {
            ZipError::NotZip => XlsxError::BadZip,
            _ => XlsxError::ZipRead,
        }
    }
}

// ─── Public spreadsheet model ──────────────────────────────────────────────

/// A single cell's typed value.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    /// Text (resolved shared-string, inline string, or `t="str"` formula string).
    Text(String),
    /// A number (default cell type). Dates are stored as serial day-numbers and are
    /// kept as `Number` — this reader does not interpret a number as a date.
    Number(f64),
    /// A boolean (`t="b"`: `1` → true, `0` → false).
    Bool(bool),
    /// An Excel error literal (`t="e"`, e.g. `#DIV/0!`, `#REF!`).
    Error(String),
    /// An empty cell (a `<c/>` with no value, or a value that could not be parsed).
    Empty,
}

impl CellValue {
    /// A human-readable rendering for the "just show me the data" path. Numbers use
    /// a compact decimal form (no trailing `.0` for integers); booleans are
    /// `TRUE`/`FALSE` (Excel's display); empty is the empty string.
    pub fn to_display_string(&self) -> String {
        match self {
            CellValue::Text(s) => s.clone(),
            CellValue::Bool(true) => String::from("TRUE"),
            CellValue::Bool(false) => String::from("FALSE"),
            CellValue::Error(s) => s.clone(),
            CellValue::Empty => String::new(),
            CellValue::Number(n) => format_number(*n),
        }
    }

    /// `true` for [`CellValue::Empty`].
    pub fn is_empty(&self) -> bool {
        matches!(self, CellValue::Empty)
    }
}

/// One cell at a grid position, with its value and (if any) formula text.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// 0-based column index (A = 0, B = 1, …, decoded from the A1 reference).
    pub col: u32,
    /// 0-based row index (the A1 reference's row number minus one).
    pub row: u32,
    /// The cell's typed value (the cached `<v>` for a formula cell).
    pub value: CellValue,
    /// The formula text from `<f>…</f>`, if this is a formula cell. The reader does
    /// not evaluate it — [`Cell::value`] holds Excel's cached result.
    pub formula: Option<String>,
}

impl Cell {
    /// `true` if this cell carries a formula (`<f>` was present).
    pub fn is_formula(&self) -> bool {
        self.formula.is_some()
    }

    /// The cell's value rendered for display (see [`CellValue::to_display_string`]).
    pub fn to_display_string(&self) -> String {
        self.value.to_display_string()
    }
}

/// One worksheet: a name plus its cells (sparse — only present cells are stored).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    /// The sheet's tab name from `xl/workbook.xml`.
    pub name: String,
    /// Cells in document order (row-major as Excel writes them). Empty cells that
    /// Excel omitted are simply absent; [`Sheet::cell`] returns `None` for them.
    pub cells: Vec<Cell>,
    /// Cached `(max_col, max_row)` exclusive dimensions (0,0 = empty sheet),
    /// computed while parsing so accessors are allocation-free.
    max_col: u32,
    max_row: u32,
    has_cells: bool,
}

impl Sheet {
    /// The value at a 0-based `(col, row)`, or `None` if that cell was empty/omitted.
    pub fn cell(&self, col: u32, row: u32) -> Option<&CellValue> {
        self.cells
            .iter()
            .find(|c| c.col == col && c.row == row)
            .map(|c| &c.value)
    }

    /// The full [`Cell`] at a 0-based `(col, row)`, or `None`.
    pub fn cell_full(&self, col: u32, row: u32) -> Option<&Cell> {
        self.cells.iter().find(|c| c.col == col && c.row == row)
    }

    /// `(width, height)` of the used range — one past the maximum populated column
    /// and row (so a single cell at A1 yields `(1, 1)`). `(0, 0)` for an empty sheet.
    pub fn dimensions(&self) -> (u32, u32) {
        if self.has_cells {
            (self.max_col + 1, self.max_row + 1)
        } else {
            (0, 0)
        }
    }

    /// Render the used range as CSV (RFC 4180-style quoting): rows separated by
    /// `\n`, fields by `,`, and a field is wrapped in double quotes (with internal
    /// `"` doubled) iff it contains a comma, quote, or newline. Empty/omitted cells
    /// render as empty fields, preserving the grid shape. This is the plain-data
    /// "show me the spreadsheet" path.
    ///
    /// ## Complexity & hostile-input bound (the load-bearing property)
    /// This walks the *populated bounding box* (`0..=max_col` × `0..=max_row`) using a
    /// one-time `(row, col)`-sorted index of [`Sheet::cells`] — it does **not** call
    /// the linear [`Sheet::cell`] accessor per grid position. So the cost is
    /// `O(N log N + populated_area)`, not the old `O(width · height · N)`. A crafted
    /// sub-1 KB sheet with two valid cells at `A1` and `XFD1048576` implies a
    /// 16384 × 1048576 = ~17.18 **billion**-position rectangle; that is past
    /// [`MAX_CSV_CELLS`], so this **infallible** variant renders nothing past the
    /// budget — it returns the empty string rather than hanging for hours. Callers
    /// that want to distinguish "empty sheet" from "refused, too large" should use
    /// [`Sheet::try_to_csv`] (which returns [`XlsxError::GridTooLarge`]).
    pub fn to_csv(&self) -> String {
        self.try_to_csv().unwrap_or_default()
    }

    /// Like [`Sheet::to_csv`], but returns [`XlsxError::GridTooLarge`] when the
    /// populated bounding box would exceed [`MAX_CSV_CELLS`] grid positions instead of
    /// silently rendering nothing. Identical CSV output to [`Sheet::to_csv`] for every
    /// sheet within budget. `O(N log N + populated_area)`; never hangs, never panics.
    pub fn try_to_csv(&self) -> Result<String, XlsxError> {
        if !self.has_cells {
            return Ok(String::new());
        }
        let w = (self.max_col as u64) + 1;
        let h = (self.max_row as u64) + 1;
        // Reject the multi-billion-cell rectangle a lone extreme cell implies BEFORE
        // materializing it. This is the DoS backstop: the area is the only quantity
        // unbounded by MAX_CELLS / MAX_COL / MAX_ROW individually.
        let area = w.saturating_mul(h);
        if area > MAX_CSV_CELLS {
            return Err(XlsxError::GridTooLarge);
        }

        // Build a one-time index sorted by (row, col) so the grid walk is a linear
        // merge against the sparse cells instead of an O(N) scan per position.
        let mut order: Vec<usize> = (0..self.cells.len()).collect();
        order.sort_by(|&a, &b| {
            let ca = &self.cells[a];
            let cb = &self.cells[b];
            (ca.row, ca.col).cmp(&(cb.row, cb.col))
        });

        let mut out = String::new();
        let mut idx = 0usize; // cursor into `order`
        let max_col = self.max_col;
        let max_row = self.max_row;
        for r in 0..=max_row {
            if r > 0 {
                out.push('\n');
            }
            for c in 0..=max_col {
                if c > 0 {
                    out.push(',');
                }
                // Advance the cursor past any cells that precede (r, c). With a clean,
                // duplicate-free sheet `order` is strictly increasing, so each cell is
                // visited once across the whole walk → O(populated_area + N) total.
                while idx < order.len() {
                    let cell = &self.cells[order[idx]];
                    if cell.row < r || (cell.row == r && cell.col < c) {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                let matched = idx < order.len() && {
                    let cell = &self.cells[order[idx]];
                    cell.row == r && cell.col == c
                };
                if matched {
                    let field = self.cells[order[idx]].value.to_display_string();
                    push_csv_field(&mut out, &field);
                    // Do not advance here: a later duplicate at the same (r,c) (a
                    // malformed sheet) is simply skipped by the cursor on the next
                    // position; the first occurrence wins, matching the linear
                    // `cell()` accessor's `find` semantics.
                }
                // Empty/omitted position → empty field (already separated above).
            }
        }
        Ok(out)
    }
}

/// A parsed SpreadsheetML workbook.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workbook {
    /// Worksheets in workbook (tab) order.
    pub sheets: Vec<Sheet>,
}

impl Workbook {
    /// Open an `.xlsx` from its full byte slice.
    ///
    /// Reads the ZIP central directory via [`ath_zip`], requires `xl/workbook.xml`
    /// and at least one resolvable worksheet, decompresses the parts (size-capped),
    /// interns shared strings, and parses each worksheet's cells. Returns `Err` on
    /// any malformed input; never panics.
    pub fn open(data: &[u8]) -> Result<Workbook, XlsxError> {
        let archive = Archive::open(data).map_err(XlsxError::from)?;

        // OPC sanity: [Content_Types].xml is mandatory in a real package; we only
        // require it to decompress if present (workbook.xml is the authoritative
        // XLSX marker below).
        if let Some(ct) = archive.find("[Content_Types].xml") {
            let _ = archive.read_entry(ct).map_err(XlsxError::from)?;
        }

        // --- xl/workbook.xml: the authoritative sheet list (name + r:id order).
        let wb_xml = read_part(&archive, "xl/workbook.xml")?.ok_or(XlsxError::NotXlsx)?;
        let sheet_refs = parse_workbook(&wb_xml)?;
        if sheet_refs.is_empty() {
            return Err(XlsxError::NotXlsx);
        }

        // --- xl/_rels/workbook.xml.rels: rId → target. Optional; if absent we fall
        // back to positional sheetN.xml resolution.
        let rels = match read_part(&archive, "xl/_rels/workbook.xml.rels")? {
            Some(x) => parse_rels(&x)?,
            None => Vec::new(),
        };

        // --- xl/sharedStrings.xml: the shared-string table (optional).
        let shared = match read_part(&archive, "xl/sharedStrings.xml")? {
            Some(x) => parse_shared_strings(&x)?,
            None => Vec::new(),
        };

        // --- Resolve + parse each worksheet in workbook order.
        let mut sheets = Vec::new();
        for (idx, sref) in sheet_refs.iter().enumerate() {
            let target = resolve_sheet_target(&rels, &sref.rid, idx);
            let path = normalize_target(&target);
            let body = match read_part(&archive, &path)? {
                Some(x) => x,
                None => {
                    // Tolerate a dangling reference: skip a sheet whose part is
                    // missing rather than failing the whole workbook.
                    continue;
                }
            };
            let mut sheet = parse_worksheet(&body, &shared)?;
            sheet.name = sref.name.clone();
            sheets.push(sheet);
        }

        if sheets.is_empty() {
            return Err(XlsxError::NotXlsx);
        }
        Ok(Workbook { sheets })
    }

    /// The sheet names, in workbook (tab) order.
    pub fn sheet_names(&self) -> Vec<&str> {
        self.sheets.iter().map(|s| s.name.as_str()).collect()
    }

    /// Find a sheet by exact name.
    pub fn sheet(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|s| s.name == name)
    }
}

/// Read a part by name, enforcing the per-part size cap and returning `Ok(None)` if
/// the entry is absent. `Err` on a decompression failure or oversize.
fn read_part(archive: &Archive, name: &str) -> Result<Option<String>, XlsxError> {
    let entry = match archive.find(name) {
        Some(e) => e,
        None => return Ok(None),
    };
    if entry.size > MAX_PART_XML {
        return Err(XlsxError::TooLarge);
    }
    let bytes = archive.read_entry(entry).map_err(XlsxError::from)?;
    let s = core::str::from_utf8(&bytes).map_err(|_| XlsxError::BadXml)?;
    Ok(Some(String::from(s)))
}

// ─── Workbook / rels / sheet-target resolution ──────────────────────────────

/// A workbook's `<sheet name r:id>` reference, in order.
struct SheetRef {
    name: String,
    rid: String,
}

/// One `<Relationship Id Target>` from the rels part.
struct Rel {
    id: String,
    target: String,
}

/// Parse `xl/workbook.xml` → ordered `<sheet name r:id>` references.
fn parse_workbook(xml: &str) -> Result<Vec<SheetRef>, XlsxError> {
    let mut tk = Tokenizer::new(xml);
    let mut budget = ElementBudget::new();
    let mut out = Vec::new();
    loop {
        match tk.next()? {
            None => break,
            Some(Token::Start { name, attrs }) | Some(Token::Empty { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "sheet" {
                    let nm = attr_value(attrs, "name").unwrap_or_default();
                    // The relationship id is the `r:id` attribute (local name "id").
                    let rid = attr_value(attrs, "id").unwrap_or_default();
                    if out.len() >= MAX_SHEETS {
                        return Err(XlsxError::TooLarge);
                    }
                    out.push(SheetRef { name: nm, rid });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Parse `xl/_rels/workbook.xml.rels` → `<Relationship Id Target>` list.
fn parse_rels(xml: &str) -> Result<Vec<Rel>, XlsxError> {
    let mut tk = Tokenizer::new(xml);
    let mut budget = ElementBudget::new();
    let mut out = Vec::new();
    loop {
        match tk.next()? {
            None => break,
            Some(Token::Start { name, attrs }) | Some(Token::Empty { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "Relationship" {
                    let id = attr_value(attrs, "Id").unwrap_or_default();
                    let target = attr_value(attrs, "Target").unwrap_or_default();
                    if out.len() >= MAX_SHEETS {
                        return Err(XlsxError::TooLarge);
                    }
                    out.push(Rel { id, target });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Resolve a sheet's `r:id` to its worksheet target via the rels list, falling back
/// to a positional `xl/worksheets/sheetN.xml` when the id is unknown/absent.
fn resolve_sheet_target(rels: &[Rel], rid: &str, idx: usize) -> String {
    if !rid.is_empty() {
        if let Some(r) = rels.iter().find(|r| r.id == rid) {
            return r.target.clone();
        }
    }
    // Positional fallback (1-based file name).
    let mut s = String::from("worksheets/sheet");
    s.push_str(&u32_to_string((idx as u32) + 1));
    s.push_str(".xml");
    s
}

/// Normalize a rels Target (relative to `xl/`) into a full archive path.
/// `"worksheets/sheet1.xml"` → `"xl/worksheets/sheet1.xml"`; a leading `/` is an
/// absolute package path; a leading `../` is collapsed against `xl/`.
fn normalize_target(target: &str) -> String {
    let t = target;
    if let Some(abs) = t.strip_prefix('/') {
        return String::from(abs);
    }
    if let Some(up) = t.strip_prefix("../") {
        // Relative to xl/ then up one → package root.
        return String::from(up);
    }
    let mut s = String::from("xl/");
    s.push_str(t);
    s
}

// ─── Shared strings ─────────────────────────────────────────────────────────

/// Parse `xl/sharedStrings.xml` → the interned string table (index → text).
/// Each `<si>` is one entry: either a single `<t>` or rich-text `<r><t>` runs
/// concatenated.
fn parse_shared_strings(xml: &str) -> Result<Vec<String>, XlsxError> {
    let mut tk = Tokenizer::new(xml);
    let mut budget = ElementBudget::new();
    let mut out = Vec::new();
    loop {
        match tk.next()? {
            None => break,
            Some(Token::Start { name, .. }) => {
                budget.spend()?;
                if local_name(name) == "si" {
                    if out.len() >= MAX_SHARED_STRINGS {
                        return Err(XlsxError::TooLarge);
                    }
                    let s = parse_si(&mut tk, &mut budget, 1)?;
                    out.push(s);
                }
            }
            Some(Token::Empty { name, .. }) => {
                budget.spend()?;
                // An empty <si/> is an empty string entry.
                if local_name(name) == "si" {
                    if out.len() >= MAX_SHARED_STRINGS {
                        return Err(XlsxError::TooLarge);
                    }
                    out.push(String::new());
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Parse the body of a `<si>` (caller consumed the start tag): concatenate every
/// `<t>` it contains, whether a direct child or inside rich-text `<r>` runs.
fn parse_si(
    tk: &mut Tokenizer,
    budget: &mut ElementBudget,
    depth: usize,
) -> Result<String, XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    let mut text = String::new();
    loop {
        match tk.next()? {
            None => return Ok(text),
            Some(Token::End { name }) if local_name(name) == "si" => return Ok(text),
            Some(Token::Start { name, attrs }) => {
                budget.spend()?;
                match local_name(name) {
                    "t" => {
                        let preserve = attr_value(attrs, "space").as_deref() == Some("preserve");
                        let raw = read_text_content(tk, "t")?;
                        let decoded = decode_entities(&raw);
                        if preserve {
                            text.push_str(&decoded);
                        } else {
                            text.push_str(decoded.trim_matches(is_xml_ws));
                        }
                    }
                    "r" => {
                        // Rich-text run: recurse to gather its <t> (treat as a mini-si).
                        let run = parse_run_text(tk, budget, depth + 1)?;
                        text.push_str(&run);
                    }
                    other => {
                        skip_element(tk, other, budget, depth + 1)?;
                    }
                }
            }
            Some(Token::Empty { .. }) => {
                budget.spend()?;
            }
            _ => {}
        }
    }
}

/// Gather the `<t>` text inside a rich-text `<r>` run (caller consumed `<r>`).
fn parse_run_text(
    tk: &mut Tokenizer,
    budget: &mut ElementBudget,
    depth: usize,
) -> Result<String, XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    let mut text = String::new();
    loop {
        match tk.next()? {
            None => return Ok(text),
            Some(Token::End { name }) if local_name(name) == "r" => return Ok(text),
            Some(Token::Start { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "t" {
                    let preserve = attr_value(attrs, "space").as_deref() == Some("preserve");
                    let raw = read_text_content(tk, "t")?;
                    let decoded = decode_entities(&raw);
                    if preserve {
                        text.push_str(&decoded);
                    } else {
                        text.push_str(decoded.trim_matches(is_xml_ws));
                    }
                } else {
                    skip_element(tk, local_name(name), budget, depth + 1)?;
                }
            }
            Some(Token::Empty { .. }) => {
                budget.spend()?;
            }
            _ => {}
        }
    }
}

// ─── Worksheet parsing ──────────────────────────────────────────────────────

/// Parse one `xl/worksheets/sheetN.xml` into a [`Sheet`] (name set by the caller).
fn parse_worksheet(xml: &str, shared: &[String]) -> Result<Sheet, XlsxError> {
    let mut tk = Tokenizer::new(xml);
    let mut budget = ElementBudget::new();
    let mut sheet = Sheet::default();

    // Walk to <sheetData>, then parse rows. Anything else (dimension, cols, …) is
    // skipped.
    loop {
        match tk.next()? {
            None => break,
            Some(Token::Start { name, .. }) => {
                budget.spend()?;
                if local_name(name) == "sheetData" {
                    parse_sheet_data(&mut tk, &mut sheet, shared, &mut budget, 1)?;
                    break;
                }
            }
            Some(Token::Empty { name, .. }) => {
                budget.spend()?;
                // An empty <sheetData/> = empty sheet.
                if local_name(name) == "sheetData" {
                    break;
                }
            }
            _ => {}
        }
    }
    Ok(sheet)
}

/// Parse `<sheetData>` children (`<row>`s) until its close.
fn parse_sheet_data(
    tk: &mut Tokenizer,
    sheet: &mut Sheet,
    shared: &[String],
    budget: &mut ElementBudget,
    depth: usize,
) -> Result<(), XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    // The 1-based row number from `<row r="N">`, used when a cell omits its own
    // reference (rare but valid); a running column counter tracks the implicit
    // position within such a row.
    loop {
        match tk.next()? {
            None => return Ok(()),
            Some(Token::End { name }) if local_name(name) == "sheetData" => return Ok(()),
            Some(Token::Start { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "row" {
                    let row_hint = attr_value(attrs, "r")
                        .as_deref()
                        .and_then(parse_u32)
                        .map(|n| n.saturating_sub(1));
                    parse_row(tk, sheet, shared, budget, depth + 1, row_hint)?;
                } else {
                    skip_element(tk, local_name(name), budget, depth + 1)?;
                }
            }
            Some(Token::Empty { .. }) => {
                budget.spend()?;
            }
            _ => {}
        }
    }
}

/// Parse one `<row>` of `<c>` cells (caller consumed `<row>`).
fn parse_row(
    tk: &mut Tokenizer,
    sheet: &mut Sheet,
    shared: &[String],
    budget: &mut ElementBudget,
    depth: usize,
    row_hint: Option<u32>,
) -> Result<(), XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    // Implicit column counter for cells lacking an `r` attribute.
    let mut implicit_col: u32 = 0;
    loop {
        match tk.next()? {
            None => return Ok(()),
            Some(Token::End { name }) if local_name(name) == "row" => return Ok(()),
            Some(Token::Start { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "c" {
                    let cell =
                        parse_cell(tk, shared, budget, depth + 1, attrs, row_hint, implicit_col)?;
                    implicit_col = cell.col.saturating_add(1);
                    push_cell(sheet, cell)?;
                } else {
                    skip_element(tk, local_name(name), budget, depth + 1)?;
                }
            }
            Some(Token::Empty { name, attrs }) => {
                budget.spend()?;
                if local_name(name) == "c" {
                    // A self-closing <c r="A1"/> is an empty cell; still placed so the
                    // grid shape is preserved.
                    let (col, row) = resolve_ref(attrs, row_hint, implicit_col)?;
                    implicit_col = col.saturating_add(1);
                    push_cell(
                        sheet,
                        Cell {
                            col,
                            row,
                            value: CellValue::Empty,
                            formula: None,
                        },
                    )?;
                }
            }
            _ => {}
        }
    }
}

/// Parse one `<c>` cell (caller consumed the start tag; `attrs` carries `r`/`t`/`s`).
fn parse_cell(
    tk: &mut Tokenizer,
    shared: &[String],
    budget: &mut ElementBudget,
    depth: usize,
    attrs: &str,
    row_hint: Option<u32>,
    implicit_col: u32,
) -> Result<Cell, XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    let (col, row) = resolve_ref(attrs, row_hint, implicit_col)?;
    let cell_type = attr_value(attrs, "t").unwrap_or_else(|| String::from("n"));

    let mut raw_v: Option<String> = None;
    let mut inline_text: Option<String> = None;
    let mut formula: Option<String> = None;

    loop {
        match tk.next()? {
            None => break,
            Some(Token::End { name }) if local_name(name) == "c" => break,
            Some(Token::Start { name, .. }) => {
                budget.spend()?;
                match local_name(name) {
                    "v" => {
                        let raw = read_text_content(tk, "v")?;
                        raw_v = Some(decode_entities(&raw));
                    }
                    "f" => {
                        let raw = read_text_content(tk, "f")?;
                        formula = Some(decode_entities(&raw));
                    }
                    "is" => {
                        // Inline string: an <is> wraps the same <t>/<r> shape as <si>.
                        let s = parse_is(tk, budget, depth + 1)?;
                        inline_text = Some(s);
                    }
                    other => {
                        skip_element(tk, other, budget, depth + 1)?;
                    }
                }
            }
            Some(Token::Empty { name, .. }) => {
                budget.spend()?;
                // A self-closing <f/> still marks the cell as a formula cell.
                if local_name(name) == "f" && formula.is_none() {
                    formula = Some(String::new());
                }
            }
            _ => {}
        }
    }

    let value = build_value(&cell_type, raw_v, inline_text, shared);
    Ok(Cell {
        col,
        row,
        value,
        formula,
    })
}

/// Parse an inline-string `<is>` body (same `<t>`/`<r>` shape as `<si>`).
fn parse_is(
    tk: &mut Tokenizer,
    budget: &mut ElementBudget,
    depth: usize,
) -> Result<String, XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    let mut text = String::new();
    loop {
        match tk.next()? {
            None => return Ok(text),
            Some(Token::End { name }) if local_name(name) == "is" => return Ok(text),
            Some(Token::Start { name, attrs }) => {
                budget.spend()?;
                match local_name(name) {
                    "t" => {
                        let preserve = attr_value(attrs, "space").as_deref() == Some("preserve");
                        let raw = read_text_content(tk, "t")?;
                        let decoded = decode_entities(&raw);
                        if preserve {
                            text.push_str(&decoded);
                        } else {
                            text.push_str(decoded.trim_matches(is_xml_ws));
                        }
                    }
                    "r" => {
                        let run = parse_run_text(tk, budget, depth + 1)?;
                        text.push_str(&run);
                    }
                    other => {
                        skip_element(tk, other, budget, depth + 1)?;
                    }
                }
            }
            Some(Token::Empty { .. }) => {
                budget.spend()?;
            }
            _ => {}
        }
    }
}

/// Build a [`CellValue`] from the cell type and its raw `<v>` / inline text.
fn build_value(
    cell_type: &str,
    raw_v: Option<String>,
    inline_text: Option<String>,
    shared: &[String],
) -> CellValue {
    match cell_type {
        "s" => {
            // Shared string: <v> is the index into the shared table.
            match raw_v.as_deref().and_then(parse_usize) {
                Some(i) => match shared.get(i) {
                    Some(s) => CellValue::Text(s.clone()),
                    None => CellValue::Empty,
                },
                None => CellValue::Empty,
            }
        }
        "inlineStr" => match inline_text {
            Some(s) => CellValue::Text(s),
            None => CellValue::Empty,
        },
        "str" => {
            // Formula string result: <v> is the literal text.
            match raw_v {
                Some(s) => CellValue::Text(s),
                None => CellValue::Empty,
            }
        }
        "b" => match raw_v.as_deref() {
            Some("1") | Some("TRUE") | Some("true") => CellValue::Bool(true),
            Some("0") | Some("FALSE") | Some("false") => CellValue::Bool(false),
            _ => CellValue::Empty,
        },
        "e" => match raw_v {
            Some(s) => CellValue::Error(s),
            None => CellValue::Empty,
        },
        // "n" (number) and any unknown/absent type default to a number.
        _ => match raw_v.as_deref().and_then(parse_f64) {
            Some(n) => CellValue::Number(n),
            None => match inline_text {
                // Some producers emit an <is> without t="inlineStr"; honor it.
                Some(s) => CellValue::Text(s),
                None => CellValue::Empty,
            },
        },
    }
}

/// Append a cell to a sheet, enforcing the per-sheet cell cap and updating the
/// cached dimensions.
fn push_cell(sheet: &mut Sheet, cell: Cell) -> Result<(), XlsxError> {
    if sheet.cells.len() >= MAX_CELLS {
        return Err(XlsxError::TooLarge);
    }
    if !sheet.has_cells {
        sheet.has_cells = true;
        sheet.max_col = cell.col;
        sheet.max_row = cell.row;
    } else {
        if cell.col > sheet.max_col {
            sheet.max_col = cell.col;
        }
        if cell.row > sheet.max_row {
            sheet.max_row = cell.row;
        }
    }
    sheet.cells.push(cell);
    Ok(())
}

/// Resolve a cell's `(col, row)` from its `r` attribute, falling back to the row
/// hint + implicit column when `r` is absent.
fn resolve_ref(
    attrs: &str,
    row_hint: Option<u32>,
    implicit_col: u32,
) -> Result<(u32, u32), XlsxError> {
    match attr_value(attrs, "r") {
        Some(r) => parse_a1(&r),
        None => {
            let row = row_hint.ok_or(XlsxError::BadCellRef)?;
            if implicit_col > MAX_COL || row >= MAX_ROW {
                return Err(XlsxError::BadCellRef);
            }
            Ok((implicit_col, row))
        }
    }
}

/// Parse an A1-style cell reference (`"B3"`, `"$AA$10"`) → 0-based `(col, row)`.
/// Leading `$` absolute markers are ignored. Returns [`XlsxError::BadCellRef`] on a
/// malformed or out-of-bounds reference. Never panics.
pub fn parse_a1(s: &str) -> Result<(u32, u32), XlsxError> {
    let bytes = s.as_bytes();
    let mut i = 0;
    // Optional column-absolute '$'.
    if i < bytes.len() && bytes[i] == b'$' {
        i += 1;
    }
    // Column letters (A..Z, case-insensitive), base-26 bijective.
    let col_start = i;
    let mut col: u32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        let v = (bytes[i].to_ascii_uppercase() - b'A') as u32 + 1;
        col = col.checked_mul(26).ok_or(XlsxError::BadCellRef)?;
        col = col.checked_add(v).ok_or(XlsxError::BadCellRef)?;
        if col > MAX_COL + 1 {
            return Err(XlsxError::BadCellRef);
        }
        i += 1;
    }
    if i == col_start {
        return Err(XlsxError::BadCellRef); // no column letters
    }
    // Optional row-absolute '$'.
    if i < bytes.len() && bytes[i] == b'$' {
        i += 1;
    }
    // Row digits (1-based).
    let row_start = i;
    let mut row: u32 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        let d = (bytes[i] - b'0') as u32;
        row = row.checked_mul(10).ok_or(XlsxError::BadCellRef)?;
        row = row.checked_add(d).ok_or(XlsxError::BadCellRef)?;
        if row > MAX_ROW {
            return Err(XlsxError::BadCellRef);
        }
        i += 1;
    }
    if i == row_start || row == 0 {
        return Err(XlsxError::BadCellRef); // no row number / row 0
    }
    if i != bytes.len() {
        return Err(XlsxError::BadCellRef); // trailing junk
    }
    // col is 1-based bijective base-26; convert to 0-based index.
    Ok((col - 1, row - 1))
}

// ─── Number / integer parsing (no_std, hand-rolled, bounded) ────────────────

/// Parse a bounded base-10 unsigned integer. Returns `None` on overflow/garbage.
fn parse_u32(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(v)
}

/// Parse a bounded base-10 `usize` (shared-string index). `None` on garbage/overflow.
fn parse_usize(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut v: usize = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as usize)?;
    }
    Some(v)
}

/// Parse a decimal `f64` from a SpreadsheetML `<v>` (plain decimal or scientific
/// notation, e.g. `"42"`, `"-3.14"`, `"6.022e23"`).
///
/// `core` has no `f64: FromStr`, so this is a hand-rolled, bounded parser:
///   - optional sign,
///   - an integer part and optional `.fraction` accumulated as an `i64` mantissa
///     with a base-10 exponent (so common values stay exact), and
///   - an optional `e`/`E` exponent.
/// The mantissa/exponent are combined via repeated `*10` / `/10` scaling. Digits
/// past `i64` precision are dropped from the mantissa (with the decimal exponent
/// adjusted), which is ample for spreadsheet values; the result is `None` on any
/// non-numeric input. Never panics.
fn parse_f64(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    let n = b.len();
    if n == 0 {
        return None;
    }
    let neg = match b[i] {
        b'+' => {
            i += 1;
            false
        }
        b'-' => {
            i += 1;
            true
        }
        _ => false,
    };

    let mut mantissa: i64 = 0;
    let mut exp10: i32 = 0; // power-of-ten the mantissa must still be scaled by
    let mut saw_digit = false;
    // Integer part.
    while i < n && b[i].is_ascii_digit() {
        saw_digit = true;
        let d = (b[i] - b'0') as i64;
        if mantissa < (i64::MAX - 9) / 10 {
            mantissa = mantissa * 10 + d;
        } else {
            // Mantissa saturated: keep magnitude via exponent, drop the digit.
            exp10 += 1;
        }
        i += 1;
    }
    // Fraction part.
    if i < n && b[i] == b'.' {
        i += 1;
        while i < n && b[i].is_ascii_digit() {
            saw_digit = true;
            let d = (b[i] - b'0') as i64;
            if mantissa < (i64::MAX - 9) / 10 {
                mantissa = mantissa * 10 + d;
                exp10 -= 1;
            }
            // else: digit beyond precision, ignored.
            i += 1;
        }
    }
    if !saw_digit {
        return None;
    }
    // Exponent part.
    if i < n && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        let esign = match b.get(i) {
            Some(b'+') => {
                i += 1;
                1
            }
            Some(b'-') => {
                i += 1;
                -1
            }
            _ => 1,
        };
        let mut e: i32 = 0;
        let estart = i;
        while i < n && b[i].is_ascii_digit() {
            e = e.saturating_mul(10).saturating_add((b[i] - b'0') as i32);
            i += 1;
        }
        if i == estart {
            return None; // 'e' with no exponent digits
        }
        exp10 = exp10.saturating_add(esign * e);
    }
    if i != n {
        return None; // trailing garbage
    }

    let mut val = mantissa as f64;
    // Scale by 10^exp10 without powi (no_std-safe loop, bounded).
    if exp10 > 0 {
        let mut k = exp10.min(308); // f64 range guard
        while k > 0 {
            val *= 10.0;
            k -= 1;
        }
        if exp10 > 308 {
            val = f64::INFINITY;
        }
    } else if exp10 < 0 {
        let mut k = (-exp10).min(308);
        while k > 0 {
            val /= 10.0;
            k -= 1;
        }
        if -exp10 > 308 {
            val = 0.0;
        }
    }
    if neg {
        val = -val;
    }
    Some(val)
}

/// Render an `f64` compactly: an integer value prints without a fractional part;
/// otherwise a bounded decimal expansion (up to 10 fractional digits, trailing
/// zeros trimmed). `no_std`-safe (no `format!`/`{}` float formatter). Non-finite
/// values render as `NaN`/`inf`/`-inf`.
fn format_number(n: f64) -> String {
    if n.is_nan() {
        return String::from("NaN");
    }
    if n.is_infinite() {
        return String::from(if n < 0.0 { "-inf" } else { "inf" });
    }
    let mut out = String::new();
    let mut x = n;
    if x < 0.0 {
        out.push('-');
        x = -x;
    }
    // Integer part via repeated division would lose precision for large values;
    // instead split on the truncated integer.
    let int_part = libm_trunc(x);
    let frac_part = x - int_part;

    // Integer digits.
    push_u64_decimal(&mut out, int_part as u64);

    if frac_part > 0.0 {
        // Up to 10 fractional digits.
        let mut frac = frac_part;
        let mut digits = String::new();
        for _ in 0..10 {
            frac *= 10.0;
            let d = libm_trunc(frac);
            let di = d as u64;
            // Guard against floating drift pushing a digit to 10.
            let di = if di > 9 { 9 } else { di };
            digits.push((b'0' + di as u8) as char);
            frac -= d;
            if frac <= 0.0 {
                break;
            }
        }
        // Trim trailing zeros.
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

/// `trunc` toward zero for a non-negative finite `f64` (no_std; soft-float kernel).
fn libm_trunc(x: f64) -> f64 {
    // For the magnitudes spreadsheets carry, casting through u64 is exact enough;
    // values beyond u64 are clamped (display-only path).
    if x >= 18_446_744_073_709_551_615.0 {
        return x; // already integral at this magnitude
    }
    (x as u64) as f64
}

/// Append a `u64` in decimal to a string (no allocation of an intermediate).
fn push_u64_decimal(out: &mut String, mut v: u64) {
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
    for &c in &buf[i..] {
        out.push(c as char);
    }
}

/// Convert a small `u32` to a decimal `String`.
fn u32_to_string(v: u32) -> String {
    let mut s = String::new();
    push_u64_decimal(&mut s, v as u64);
    s
}

// ─── CSV field quoting (RFC 4180) ───────────────────────────────────────────

/// Append `field` to `out` as a CSV field, quoting iff it contains a comma, double
/// quote, CR, or LF (internal quotes doubled).
fn push_csv_field(out: &mut String, field: &str) {
    let needs_quote = field
        .bytes()
        .any(|b| b == b',' || b == b'"' || b == b'\n' || b == b'\r');
    if !needs_quote {
        out.push_str(field);
        return;
    }
    out.push('"');
    for ch in field.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
}

// ════════════════════════════════════════════════════════════════════════════
// Minimal XML tokenizer (identical strategy to ath_docx — well-formed,
// machine-generated parts). Bounds-checked byte cursor; no panic path.
// ════════════════════════════════════════════════════════════════════════════

/// One XML token.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token<'a> {
    Start { name: &'a str, attrs: &'a str },
    End { name: &'a str },
    Empty { name: &'a str, attrs: &'a str },
    Text(&'a str),
    Cdata(&'a str),
}

/// A streaming, bounds-checked XML tokenizer over a `&str`.
struct Tokenizer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(src: &'a str) -> Self {
        Tokenizer {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    fn next(&mut self) -> Result<Option<Token<'a>>, XlsxError> {
        if self.pos >= self.bytes.len() {
            return Ok(None);
        }
        if self.bytes[self.pos] == b'<' {
            self.read_tag()
        } else {
            self.read_text()
        }
    }

    fn read_text(&mut self) -> Result<Option<Token<'a>>, XlsxError> {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'<' {
            self.pos += 1;
        }
        Ok(Some(Token::Text(&self.src[start..self.pos])))
    }

    fn read_tag(&mut self) -> Result<Option<Token<'a>>, XlsxError> {
        if self.starts_with(b"<!--") {
            return self.skip_until(b"-->", 4).map(|_| Some(Token::Text("")));
        }
        if self.starts_with(b"<![CDATA[") {
            let inner_start = self.pos + 9;
            let end_rel = find_sub(&self.bytes[inner_start..], b"]]>").ok_or(XlsxError::BadXml)?;
            let inner_end = inner_start + end_rel;
            let text = &self.src[inner_start..inner_end];
            self.pos = inner_end + 3;
            return Ok(Some(Token::Cdata(text)));
        }
        if self.starts_with(b"<?") {
            return self.skip_until(b"?>", 2).map(|_| Some(Token::Text("")));
        }
        if self.starts_with(b"<!") {
            return self.skip_until(b">", 2).map(|_| Some(Token::Text("")));
        }

        let tag_start = self.pos + 1; // skip '<'
        let close_rel = find_byte(&self.bytes[tag_start..], b'>').ok_or(XlsxError::BadXml)?;
        let inner = &self.src[tag_start..tag_start + close_rel];
        self.pos = tag_start + close_rel + 1;

        if let Some(rest) = inner.strip_prefix('/') {
            let name = rest.trim();
            if name.is_empty() {
                return Err(XlsxError::BadXml);
            }
            return Ok(Some(Token::End { name }));
        }

        let (inner, empty) = match inner.strip_suffix('/') {
            Some(r) => (r, true),
            None => (inner, false),
        };
        let inner = inner.trim();
        if inner.is_empty() {
            return Err(XlsxError::BadXml);
        }
        let (name, attrs) = match inner.find(|c: char| c.is_ascii_whitespace()) {
            Some(i) => (&inner[..i], inner[i..].trim_start()),
            None => (inner, ""),
        };
        if empty {
            Ok(Some(Token::Empty { name, attrs }))
        } else {
            Ok(Some(Token::Start { name, attrs }))
        }
    }

    fn starts_with(&self, needle: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(needle)
    }

    fn skip_until(&mut self, needle: &[u8], skip: usize) -> Result<(), XlsxError> {
        let from = self.pos + skip;
        let rel =
            find_sub(self.bytes.get(from..).unwrap_or(&[]), needle).ok_or(XlsxError::BadXml)?;
        self.pos = from + rel + needle.len();
        Ok(())
    }
}

fn find_byte(hay: &[u8], b: u8) -> Option<usize> {
    hay.iter().position(|&x| x == b)
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    let mut i = 0;
    while i <= last {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ─── Element budget + tree helpers (shared with the worksheet walker) ────────

/// Track and bound the number of start-tags processed per part.
struct ElementBudget {
    remaining: usize,
}

impl ElementBudget {
    fn new() -> Self {
        ElementBudget {
            remaining: MAX_ELEMENTS,
        }
    }
    fn spend(&mut self) -> Result<(), XlsxError> {
        if self.remaining == 0 {
            return Err(XlsxError::TooLarge);
        }
        self.remaining -= 1;
        Ok(())
    }
}

/// Read raw character data inside an element up to its matching end tag of `local`,
/// returning the concatenated text. Keeps nested text but ignores nested element
/// tags (sufficient for `<t>`/`<v>`/`<f>`, which have no element children of interest).
fn read_text_content(tk: &mut Tokenizer, local: &str) -> Result<String, XlsxError> {
    let mut out = String::new();
    let mut depth = 0usize;
    loop {
        match tk.next()? {
            None => return Ok(out),
            Some(Token::Text(t)) => out.push_str(t),
            Some(Token::Cdata(t)) => out.push_str(t),
            Some(Token::End { name }) => {
                if depth == 0 && local_name(name) == local {
                    return Ok(out);
                }
                depth = depth.saturating_sub(1);
            }
            Some(Token::Start { .. }) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(XlsxError::TooDeep);
                }
            }
            Some(Token::Empty { .. }) => {}
        }
    }
}

/// Skip the remainder of an element subtree whose start tag (`local`) was already
/// consumed, balancing nested start/end tags.
fn skip_element(
    tk: &mut Tokenizer,
    local: &str,
    budget: &mut ElementBudget,
    depth: usize,
) -> Result<(), XlsxError> {
    if depth > MAX_DEPTH {
        return Err(XlsxError::TooDeep);
    }
    let mut nesting = 0usize;
    loop {
        match tk.next()? {
            None => return Ok(()),
            Some(Token::Start { .. }) => {
                budget.spend()?;
                nesting += 1;
                if nesting > MAX_DEPTH {
                    return Err(XlsxError::TooDeep);
                }
            }
            Some(Token::Empty { .. }) => {
                budget.spend()?;
            }
            Some(Token::End { name }) => {
                if nesting == 0 {
                    let _ = local;
                    let _ = local_name(name);
                    return Ok(());
                }
                nesting -= 1;
            }
            _ => {}
        }
    }
}

// ─── Small helpers (shared with ath_docx's conventions) ─────────────────────

/// Strip a `prefix:` namespace qualifier, returning the local part. `"r:id"` → `"id"`.
fn local_name(name: &str) -> &str {
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Whether a char is XML insignificant whitespace.
fn is_xml_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// Extract the value of the attribute whose *local* name is `local` from a raw
/// attribute-list slice. Handles single/double quotes and namespace-prefixed names;
/// returns the entity-decoded value. Never panics (returns `None`).
fn attr_value(attrs: &str, local: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if name_start == i {
            break;
        }
        let name = &attrs[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1; // '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            let vstart = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if local_name(name) == local {
                return Some(decode_entities(&attrs[vstart..i]));
            }
            continue;
        }
        i += 1; // opening quote
        let vstart = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return None; // unterminated value
        }
        let value = &attrs[vstart..i];
        i += 1; // closing quote
        if local_name(name) == local {
            return Some(decode_entities(value));
        }
    }
    None
}

/// Decode the five predefined XML entities and numeric character references
/// (`&#NN;` decimal, `&#xHH;` hex). Unknown/malformed entities are left verbatim.
fn decode_entities(s: &str) -> String {
    if !s.as_bytes().contains(&b'&') {
        return String::from(s);
    }
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                i += 1;
            }
            out.push_str(&s[start..i]);
            continue;
        }
        let semi = match find_byte(&bytes[i..], b';') {
            Some(rel) => i + rel,
            None => {
                out.push('&');
                i += 1;
                continue;
            }
        };
        let ent = &s[i + 1..semi];
        let replacement = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => {
                if let Some(num) = ent.strip_prefix('#') {
                    let code = if let Some(hex) =
                        num.strip_prefix('x').or_else(|| num.strip_prefix('X'))
                    {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    code.and_then(char::from_u32)
                } else {
                    None
                }
            }
        };
        match replacement {
            Some(c) => {
                out.push(c);
                i = semi + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

mod writer;
pub use writer::{WorkbookBuilder, XlsxWriteError};

mod formula;
pub use formula::{
    eval_formula, evaluate, MAX_EVAL_CELLS, MAX_EXPR_DEPTH, MAX_FORMULA_LEN, MAX_RANGE_CELLS,
};

/// Crate-internal re-export of the bounded `f64` parser for the formula evaluator
/// (it needs the exact same numeric-coercion semantics the reader uses for `<v>`).
pub(crate) fn parse_f64_pub(s: &str) -> Option<f64> {
    parse_f64(s)
}

#[cfg(test)]
mod tests;
