//! # RaeCSV — a never-panic, `no_std` CSV parser + serializer (RFC 4180).
//!
//! RaeenOS_Concept.md §"the user owns the machine" — your data files are *yours*:
//! a spreadsheet, an export, a log dumped to comma-separated values opens with no
//! cloud round-trip, no account, no friction. The concrete daily-driver gap this
//! closes is "open my `.csv` and see a table": one correct, dependency-free,
//! hostile-input CSV core that a Files Quick Look panel, a lightweight spreadsheet
//! viewer, and settings import/export can all share. It is deliberately wired into
//! none of them this slice — it is foundational infrastructure, not glue.
//!
//! ## Hostile-input posture (CLAUDE §10: parsers of untrusted bytes are a surface)
//! Every byte handed to [`parse`] is treated as hostile. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from the parser:
//! unterminated quoted fields, lone quotes, stray quotes in the middle of an
//! unquoted field, only-delimiter lines, empty input, and CRLF/LF/bare-CR mixes
//! all decode best-effort (RFC 4180 §2 is permissive about lone `"`), never
//! panicking. Pathological inputs — a single cell larger than [`MAX_CELL_BYTES`],
//! more than [`MAX_ROWS`] rows, or more than [`MAX_COLS`] columns in one row —
//! return [`CsvError`] *before* memory is exhausted. The host KAT suite at the
//! bottom of this file is the primary proof (`cargo test -p rae_csv`).
//!
//! ## What it is
//! - A [`Csv`] table: `rows: Vec<Vec<String>>`, ragged-tolerant (rows may differ
//!   in width; [`Csv::cols`] reports the max).
//! - [`parse`] / [`parse_with`]: an RFC 4180 reader over UTF-8 `&str`. Handles
//!   quoted fields (`"..."` may contain the delimiter, CR, and LF), the escaped
//!   quote `""` → `"` inside a quoted field, unquoted fields, CRLF and LF row
//!   terminators, a trailing newline (no spurious final empty row), empty fields,
//!   and explicit blank lines (→ a one-empty-cell row). The delimiter is
//!   configurable (comma default; `\t` for TSV, `;` for EU-CSV).
//! - [`to_string`] / [`to_string_with`]: a round-trippable writer. A field is
//!   quoted iff it contains the delimiter, a quote, CR, or LF; internal quotes are
//!   doubled; rows are LF-terminated (documented choice — see [`to_string`]).
//! - Panic-free accessors ([`Csv::cell`], [`Csv::row`], [`Csv::header`], …) and a
//!   header view ([`Csv::with_header`]).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Maximum number of bytes a single field/cell may contain before [`parse`]
/// rejects the input with [`CsvError::CellTooLarge`]. A crafted record with one
/// never-closed quoted field would otherwise grow a `String` until the allocator
/// dies; this is the per-cell memory bound for the hostile-input posture, not a
/// CSV-spec limit. 16 MiB is far past any real spreadsheet cell.
pub const MAX_CELL_BYTES: usize = 16 * 1024 * 1024;

/// Maximum number of rows [`parse`] will accept before returning
/// [`CsvError::TooManyRows`]. Bounds total row-vector growth on a pathological
/// input (millions of `\n`). 4 million rows is past any interactive table.
pub const MAX_ROWS: usize = 4 * 1024 * 1024;

/// Maximum number of columns in a single row before [`parse`] returns
/// [`CsvError::TooManyCols`]. Bounds per-row field-vector growth on a line that
/// is nothing but delimiters. 65536 columns is past any real sheet.
pub const MAX_COLS: usize = 64 * 1024;

/// A parsed CSV table: a ragged grid of cells.
///
/// Rows may have different widths (RFC 4180 §1 recommends but does not require
/// equal field counts); [`Csv::cols`] reports the widest. Cells are owned
/// `String`s with all CSV quoting already removed, so a consumer sees the literal
/// data values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Csv {
    rows: Vec<Vec<String>>,
}

/// Why parsing failed. The only failures are the memory-safety caps — malformed
/// quoting is *recovered*, not rejected (RFC 4180 readers are expected to be
/// permissive), so a viewer can always show *something* for a real-but-messy file
/// while a truly hostile (giant) input is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvError {
    /// A single field exceeded [`MAX_CELL_BYTES`] (e.g. an unterminated quoted
    /// field that swallowed the rest of a huge input).
    CellTooLarge,
    /// The table exceeded [`MAX_ROWS`] rows.
    TooManyRows,
    /// A single row exceeded [`MAX_COLS`] columns.
    TooManyCols,
    /// The chosen delimiter or quote character is not a single ASCII byte. CSV
    /// structural characters must be ASCII so they cannot appear inside a UTF-8
    /// multibyte sequence.
    BadDelimiter,
}

/// The default field delimiter (comma), per RFC 4180.
pub const COMMA: char = ',';
/// The quote character, per RFC 4180. Not configurable (always `"`).
const QUOTE: u8 = b'"';

/// Parse `input` as comma-delimited RFC 4180 CSV.
///
/// Equivalent to [`parse_with`] with [`COMMA`]. See the crate docs for the full
/// list of handled cases (quoting, `""` escape, embedded delimiter/newline, CRLF
/// vs LF, trailing newline, blank lines). Never panics.
pub fn parse(input: &str) -> Result<Csv, CsvError> {
    parse_with(input, COMMA)
}

/// Parse `input` as RFC 4180 CSV using `delimiter` as the field separator.
///
/// `delimiter` must be a single-byte ASCII character that is not `"`, CR, or LF
/// (otherwise [`CsvError::BadDelimiter`]). Common choices: `,` (CSV), `\t` (TSV),
/// `;` (European CSV). Operates byte-wise; because the delimiter, quote, CR, and
/// LF are all ASCII they can never collide with a UTF-8 continuation byte, so the
/// scan is char-boundary-safe and multibyte cell content is preserved verbatim.
///
/// ## Recovery rules (RFC 4180 §2, permissive reader)
/// - A `"` that opens a field begins a quoted field: the delimiter, CR, and LF
///   are literal until the closing `"`; `""` inside is a literal `"`.
/// - An unterminated quoted field (EOF before the closing `"`) is closed at EOF.
/// - A `"` appearing mid-unquoted-field is treated as a literal character (lenient).
/// - A blank line (no characters before the terminator) becomes a single empty
///   cell `[""]` — a present-but-empty row, distinct from end-of-input.
/// - A trailing line terminator does **not** create a spurious final empty row.
pub fn parse_with(input: &str, delimiter: char) -> Result<Csv, CsvError> {
    // Structural characters must be single-byte ASCII.
    if !delimiter.is_ascii() || delimiter == '"' || delimiter == '\r' || delimiter == '\n' {
        return Err(CsvError::BadDelimiter);
    }
    let delim = delimiter as u8;
    let bytes = input.as_bytes();

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut field_bytes: Vec<u8> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    // True once the current field has accumulated any unquoted content, so a `"`
    // appearing after that point is a literal character (lenient), not the start
    // of a quoted field. Reset on every delimiter / record terminator.
    let mut field_in_progress = false;
    // Did we consume any field/record content since the last record push? Used to
    // suppress a spurious trailing-newline empty row while still emitting a real
    // blank line as an empty row.
    let mut record_started = false;

    let mut i = 0usize;
    let len = bytes.len();

    // Finalize the in-progress field into the current record.
    // Returns Err on the cell/col caps.
    macro_rules! push_field {
        ($field:expr, $record:expr) => {{
            if $field.len() > MAX_CELL_BYTES {
                return Err(CsvError::CellTooLarge);
            }
            if $record.len() >= MAX_COLS {
                return Err(CsvError::TooManyCols);
            }
            // `field_bytes` only ever accumulates the original UTF-8 bytes of the
            // input (we copy verbatim slices), so it is always valid UTF-8.
            let s = String::from_utf8($field.clone()).unwrap_or_default();
            $record.push(s);
            $field.clear();
        }};
    }

    while i < len {
        let b = bytes[i];

        if b == QUOTE && !field_in_progress {
            // A `"` at field start opens a quoted field: consume until an
            // unescaped closing quote or EOF.
            record_started = true;
            field_in_progress = true;
            i += 1; // skip opening quote
            loop {
                if i >= len {
                    // Unterminated quoted field: close at EOF (lenient recovery).
                    break;
                }
                let c = bytes[i];
                if c == QUOTE {
                    if i + 1 < len && bytes[i + 1] == QUOTE {
                        // Escaped quote "" -> literal "
                        if field_bytes.len() >= MAX_CELL_BYTES {
                            return Err(CsvError::CellTooLarge);
                        }
                        field_bytes.push(QUOTE);
                        i += 2;
                        continue;
                    }
                    // Closing quote.
                    i += 1;
                    break;
                }
                if field_bytes.len() >= MAX_CELL_BYTES {
                    return Err(CsvError::CellTooLarge);
                }
                field_bytes.push(c);
                i += 1;
            }
            // After a closing quote, any trailing chars until the next delimiter
            // or terminator are appended literally (lenient; RFC 4180 strict CSV
            // wouldn't have them, but a viewer should not drop data).
            while i < len && bytes[i] != delim && bytes[i] != b'\r' && bytes[i] != b'\n' {
                if field_bytes.len() >= MAX_CELL_BYTES {
                    return Err(CsvError::CellTooLarge);
                }
                field_bytes.push(bytes[i]);
                i += 1;
            }
            continue;
        }

        if b == delim {
            record_started = true;
            push_field!(field_bytes, record);
            field_in_progress = false;
            i += 1;
            continue;
        }

        if b == b'\r' || b == b'\n' {
            // End of record. Consume CRLF as one terminator.
            if b == b'\r' && i + 1 < len && bytes[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
            // Emit the final field of this record, then the record itself.
            push_field!(field_bytes, record);
            if rows.len() >= MAX_ROWS {
                return Err(CsvError::TooManyRows);
            }
            // Take the record out.
            let mut done = Vec::new();
            core::mem::swap(&mut done, &mut record);
            rows.push(done);
            record_started = false;
            field_in_progress = false;
            continue;
        }

        // Plain byte of an unquoted field.
        record_started = true;
        field_in_progress = true;
        if field_bytes.len() >= MAX_CELL_BYTES {
            return Err(CsvError::CellTooLarge);
        }
        field_bytes.push(b);
        i += 1;
    }

    // Flush a final record that had no trailing terminator (so a file without a
    // final newline keeps its last row), but DON'T emit a phantom empty row for a
    // file that *did* end with a terminator.
    if record_started || !record.is_empty() || !field_bytes.is_empty() {
        push_field!(field_bytes, record);
        if rows.len() >= MAX_ROWS {
            return Err(CsvError::TooManyRows);
        }
        rows.push(record);
    }

    Ok(Csv { rows })
}

impl Csv {
    /// Construct a table directly from owned rows (e.g. to serialize generated
    /// data). Does not validate against the caps — those guard *parsing* of
    /// untrusted input; locally-built tables are trusted.
    pub fn from_rows(rows: Vec<Vec<String>>) -> Csv {
        Csv { rows }
    }

    /// All rows of the table.
    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the table has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Row `i`, or `None` if out of range.
    pub fn row(&self, i: usize) -> Option<&Vec<String>> {
        self.rows.get(i)
    }

    /// Cell at (`r`, `c`) as a `&str`, or `None` if either index is out of range.
    pub fn cell(&self, r: usize, c: usize) -> Option<&str> {
        self.rows
            .get(r)
            .and_then(|row| row.get(c))
            .map(|s| s.as_str())
    }

    /// The number of columns — the width of the widest row (the grid is ragged).
    pub fn cols(&self) -> usize {
        let mut max = 0;
        for row in &self.rows {
            if row.len() > max {
                max = row.len();
            }
        }
        max
    }

    /// The first row, treated as a header, or `None` for an empty table.
    pub fn header(&self) -> Option<&[String]> {
        self.rows.first().map(|r| r.as_slice())
    }

    /// A header-aware view: the first row is the header, the rest are records.
    /// Returns `None` for an empty table.
    pub fn with_header(&self) -> Option<CsvWithHeader<'_>> {
        if self.rows.is_empty() {
            None
        } else {
            Some(CsvWithHeader { csv: self })
        }
    }
}

/// A view over a [`Csv`] whose first row is interpreted as column headers.
#[derive(Debug, Clone, Copy)]
pub struct CsvWithHeader<'a> {
    csv: &'a Csv,
}

impl<'a> CsvWithHeader<'a> {
    /// The header row.
    pub fn header(&self) -> &'a [String] {
        // Constructed only when `rows` is non-empty.
        self.csv.rows.first().map(|r| r.as_slice()).unwrap_or(&[])
    }

    /// The data rows (everything after the header).
    pub fn records(&self) -> &'a [Vec<String>] {
        if self.csv.rows.len() > 1 {
            &self.csv.rows[1..]
        } else {
            &[]
        }
    }

    /// The number of data records (rows excluding the header).
    pub fn record_count(&self) -> usize {
        self.csv.rows.len().saturating_sub(1)
    }

    /// The zero-based column index of `name` in the header, if present.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.header().iter().position(|h| h == name)
    }

    /// The value of column `name` in data record `record` (0-based, header
    /// excluded), if both the column and the cell exist.
    pub fn get(&self, record: usize, name: &str) -> Option<&'a str> {
        let col = self.column_index(name)?;
        self.records()
            .get(record)
            .and_then(|r| r.get(col))
            .map(|s| s.as_str())
    }
}

/// Serialize a [`Csv`] back to a comma-delimited RFC 4180 string with LF row
/// terminators. See [`to_string_with`].
pub fn to_string(csv: &Csv) -> String {
    to_string_with(csv, COMMA)
}

/// Serialize a [`Csv`] using `delimiter` as the field separator.
///
/// A field is quoted iff it contains the `delimiter`, a `"`, a CR, or an LF;
/// internal `"` are doubled. Rows are terminated with a single `\n` (LF) — the
/// documented choice; this keeps output small and round-trips through [`parse`],
/// which accepts both LF and CRLF. The final row is LF-terminated, which [`parse`]
/// does *not* read back as a spurious empty row.
///
/// If `delimiter` is not a single ASCII byte (or is `"`/CR/LF) it falls back to
/// [`COMMA`] rather than panicking — serialization never fails.
pub fn to_string_with(csv: &Csv, delimiter: char) -> String {
    let delim =
        if delimiter.is_ascii() && delimiter != '"' && delimiter != '\r' && delimiter != '\n' {
            delimiter
        } else {
            COMMA
        };

    let mut out = String::new();
    for row in &csv.rows {
        let mut first = true;
        for field in row {
            if !first {
                out.push(delim);
            }
            first = false;
            write_field(&mut out, field, delim);
        }
        out.push('\n');
    }
    out
}

/// Write one field, quoting and escaping as needed.
fn write_field(out: &mut String, field: &str, delim: char) {
    let needs_quote = field
        .bytes()
        .any(|b| b == QUOTE || b == b'\r' || b == b'\n' || b == delim as u8);
    if !needs_quote {
        out.push_str(field);
        return;
    }
    out.push('"');
    for c in field.chars() {
        if c == '"' {
            out.push('"');
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out.push('"');
}

// ===========================================================================
// Host KAT suite — the FAIL-able proof. `cargo test -p rae_csv`.
//
// TEST-MODULE rule (CLAUDE / R7 gate): under `cfg(test)` this crate builds as
// `std`, but we still use `alloc::…` / the implicit prelude and NEVER `use std::`
// / `extern crate std`, so the no_std production build and the test build share
// the exact same code paths.
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn cells(csv: &Csv) -> Vec<Vec<&str>> {
        csv.rows()
            .iter()
            .map(|r| r.iter().map(|s| s.as_str()).collect())
            .collect()
    }

    #[test]
    fn simple_grid() {
        let csv = parse("a,b,c\n1,2,3\n").unwrap();
        assert_eq!(csv.len(), 2);
        assert_eq!(csv.cols(), 3);
        assert_eq!(csv.cell(0, 0), Some("a"));
        assert_eq!(csv.cell(0, 2), Some("c"));
        assert_eq!(csv.cell(1, 1), Some("2"));
        // FAIL-ability: if the row splitter dropped a field these guards flip.
        assert_ne!(csv.row(0).map(|r| r.len()), Some(2));
        assert_ne!(csv.cell(1, 1), Some("3"));
        assert_eq!(csv.cell(9, 9), None);
    }

    #[test]
    fn quoted_embedded_delimiter() {
        // "x,y",z  must be TWO fields, the first literally `x,y`.
        let csv = parse("\"x,y\",z").unwrap();
        assert_eq!(csv.len(), 1);
        assert_eq!(csv.row(0).map(|r| r.len()), Some(2));
        assert_eq!(csv.cell(0, 0), Some("x,y"));
        assert_eq!(csv.cell(0, 1), Some("z"));
        // FAIL-ability: broken quoted-comma logic would split into 3 fields and
        // cell(0,0) would be "x — this assert_ne is the trip wire.
        assert_ne!(csv.row(0).map(|r| r.len()), Some(3));
        assert_ne!(csv.cell(0, 0), Some("x"));
    }

    #[test]
    fn escaped_quote() {
        // "she said ""hi"""  ->  she said "hi"
        let csv = parse("\"she said \"\"hi\"\"\"").unwrap();
        assert_eq!(csv.cell(0, 0), Some("she said \"hi\""));
        // FAIL-ability: if the "" escape were mishandled the cell would be
        // `she said hi` or split into multiple fields — this guard catches it.
        assert_ne!(csv.cell(0, 0), Some("she said hi"));
        assert_eq!(csv.row(0).map(|r| r.len()), Some(1));
    }

    #[test]
    fn embedded_newline_in_quoted_field() {
        let csv = parse("\"line1\nline2\",b\n").unwrap();
        assert_eq!(csv.len(), 1);
        assert_eq!(csv.cell(0, 0), Some("line1\nline2"));
        assert_eq!(csv.cell(0, 1), Some("b"));
        // FAIL-ability: if quoted newlines split records we'd get 2 rows.
        assert_ne!(csv.len(), 2);
    }

    #[test]
    fn embedded_crlf_in_quoted_field() {
        let csv = parse("\"a\r\nb\",c\n").unwrap();
        assert_eq!(csv.cell(0, 0), Some("a\r\nb"));
        assert_eq!(csv.cell(0, 1), Some("c"));
        assert_eq!(csv.len(), 1);
    }

    #[test]
    fn crlf_and_lf_decode_identically() {
        let lf = parse("a,b\n1,2\n").unwrap();
        let crlf = parse("a,b\r\n1,2\r\n").unwrap();
        assert_eq!(lf, crlf);
        assert_eq!(crlf.len(), 2);
    }

    #[test]
    fn trailing_newline_no_phantom_row() {
        let with = parse("a,b\n").unwrap();
        let without = parse("a,b").unwrap();
        assert_eq!(with.len(), 1);
        assert_eq!(without.len(), 1);
        assert_eq!(with, without);
        // FAIL-ability: a phantom empty final row would make len()==2.
        assert_ne!(with.len(), 2);
    }

    #[test]
    fn explicit_blank_line_is_empty_row() {
        // a\n\nb  -> three rows: ["a"], [""], ["b"]
        let csv = parse("a\n\nb").unwrap();
        assert_eq!(csv.len(), 3);
        assert_eq!(cells(&csv), vec![vec!["a"], vec![""], vec!["b"]]);
    }

    #[test]
    fn empty_fields() {
        let csv = parse("a,,c\n,,\n").unwrap();
        assert_eq!(csv.cell(0, 1), Some(""));
        assert_eq!(cells(&csv), vec![vec!["a", "", "c"], vec!["", "", ""]]);
    }

    #[test]
    fn tsv_delimiter() {
        let csv = parse_with("a\tb\tc\n1\t2\t3", '\t').unwrap();
        assert_eq!(csv.cols(), 3);
        assert_eq!(csv.cell(1, 2), Some("3"));
        // A comma inside a TSV field is just data, not a separator.
        let csv2 = parse_with("x,y\tz", '\t').unwrap();
        assert_eq!(csv2.cell(0, 0), Some("x,y"));
        assert_eq!(csv2.row(0).map(|r| r.len()), Some(2));
    }

    #[test]
    fn semicolon_delimiter() {
        let csv = parse_with("a;b;c", ';').unwrap();
        assert_eq!(csv.cols(), 3);
        assert_eq!(csv.cell(0, 1), Some("b"));
    }

    #[test]
    fn round_trip_tricky() {
        // Quotes, commas, and newlines in cells — the hard case.
        let input = "name,note\n\"Smith, J.\",\"said \"\"hi\"\"\nthen left\"\nbob,plain\n";
        let parsed = parse(input).unwrap();
        let serialized = to_string(&parsed);
        let reparsed = parse(&serialized).unwrap();
        assert_eq!(parsed, reparsed);
        // Spot-check the load-bearing cells survived the round trip.
        assert_eq!(reparsed.cell(1, 0), Some("Smith, J."));
        assert_eq!(reparsed.cell(1, 1), Some("said \"hi\"\nthen left"));
        assert_eq!(reparsed.cell(2, 0), Some("bob"));
        // FAIL-ability: break write_field's quoting and these cells corrupt /
        // the equality fails.
        assert_ne!(reparsed.cell(1, 0), Some("Smith"));
    }

    #[test]
    fn round_trip_tsv() {
        let input = "a\tb\n\"has\ttab\"\tc\n";
        let parsed = parse_with(input, '\t').unwrap();
        let out = to_string_with(&parsed, '\t');
        let reparsed = parse_with(&out, '\t').unwrap();
        assert_eq!(parsed, reparsed);
        assert_eq!(reparsed.cell(1, 0), Some("has\ttab"));
    }

    #[test]
    fn ragged_rows() {
        let csv = parse("a,b,c\n1,2\nx\n").unwrap();
        assert_eq!(csv.cols(), 3);
        assert_eq!(csv.row(0).map(|r| r.len()), Some(3));
        assert_eq!(csv.row(1).map(|r| r.len()), Some(2));
        assert_eq!(csv.row(2).map(|r| r.len()), Some(1));
        assert_eq!(csv.cell(2, 0), Some("x"));
        assert_eq!(csv.cell(1, 2), None);
    }

    #[test]
    fn header_view() {
        let csv = parse("name,age\nalice,30\nbob,25\n").unwrap();
        assert_eq!(
            csv.header(),
            Some(&["name".to_string(), "age".to_string()][..])
        );
        let view = csv.with_header().unwrap();
        assert_eq!(view.record_count(), 2);
        assert_eq!(view.column_index("age"), Some(1));
        assert_eq!(view.column_index("missing"), None);
        assert_eq!(view.get(0, "name"), Some("alice"));
        assert_eq!(view.get(1, "age"), Some("25"));
        assert_eq!(view.get(9, "name"), None);
    }

    #[test]
    fn unicode_cells_preserved() {
        // Multibyte UTF-8 must survive the byte-wise scan untouched.
        let csv = parse("café,naïve,日本語\nπ,😀,x\n").unwrap();
        assert_eq!(csv.cell(0, 0), Some("café"));
        assert_eq!(csv.cell(0, 2), Some("日本語"));
        assert_eq!(csv.cell(1, 1), Some("😀"));
    }

    // ---- Malformed / edge battery: ZERO panics, Err or graceful ----

    #[test]
    fn empty_input_is_empty_table() {
        let csv = parse("").unwrap();
        assert!(csv.is_empty());
        assert_eq!(csv.len(), 0);
        assert_eq!(csv.cols(), 0);
        assert_eq!(csv.header(), None);
        assert!(csv.with_header().is_none());
    }

    #[test]
    fn only_delimiters() {
        let csv = parse(",,,").unwrap();
        assert_eq!(csv.len(), 1);
        assert_eq!(csv.row(0).map(|r| r.len()), Some(4));
        assert_eq!(cells(&csv), vec![vec!["", "", "", ""]]);
    }

    #[test]
    fn lone_quote_recovers() {
        // A bare quote at field start opens a quoted field that runs to EOF.
        let csv = parse("\"").unwrap();
        assert_eq!(csv.len(), 1);
        assert_eq!(csv.cell(0, 0), Some(""));
    }

    #[test]
    fn unterminated_quoted_field_recovers() {
        let csv = parse("a,\"unterminated, with comma\nand newline").unwrap();
        assert_eq!(csv.len(), 1);
        assert_eq!(csv.cell(0, 0), Some("a"));
        assert_eq!(
            csv.cell(0, 1),
            Some("unterminated, with comma\nand newline")
        );
    }

    #[test]
    fn quote_mid_unquoted_field_is_literal() {
        // Lenient: a quote not at field start is data.
        let csv = parse("ab\"cd,e").unwrap();
        assert_eq!(csv.cell(0, 0), Some("ab\"cd"));
        assert_eq!(csv.cell(0, 1), Some("e"));
    }

    #[test]
    fn trailing_data_after_closing_quote() {
        // "ab"cd  -> lenient: append cd literally -> abcd
        let csv = parse("\"ab\"cd,e").unwrap();
        assert_eq!(csv.cell(0, 0), Some("abcd"));
        assert_eq!(csv.cell(0, 1), Some("e"));
    }

    #[test]
    fn bare_cr_terminator() {
        let csv = parse("a\rb").unwrap();
        assert_eq!(csv.len(), 2);
        assert_eq!(csv.cell(0, 0), Some("a"));
        assert_eq!(csv.cell(1, 0), Some("b"));
    }

    #[test]
    fn bad_delimiter_rejected() {
        assert_eq!(parse_with("a,b", '"'), Err(CsvError::BadDelimiter));
        assert_eq!(parse_with("a,b", '\n'), Err(CsvError::BadDelimiter));
        // Non-ASCII delimiter rejected (could collide with a UTF-8 cont byte).
        assert_eq!(parse_with("a,b", '€'), Err(CsvError::BadDelimiter));
    }

    #[test]
    fn huge_cell_past_cap_errors() {
        // One unterminated quoted field larger than MAX_CELL_BYTES must Err, not
        // OOM or panic. Build it cheaply.
        let mut big = String::with_capacity(MAX_CELL_BYTES + 16);
        big.push('"');
        for _ in 0..(MAX_CELL_BYTES + 8) {
            big.push('x');
        }
        assert_eq!(parse(&big), Err(CsvError::CellTooLarge));
    }

    #[test]
    fn to_string_quotes_only_when_needed() {
        let csv = Csv::from_rows(vec![
            vec!["plain".to_string(), "has,comma".to_string()],
            vec!["has\"quote".to_string(), "has\nnewline".to_string()],
        ]);
        let out = to_string(&csv);
        // plain is unquoted; the others are quoted.
        assert!(out.starts_with("plain,\"has,comma\"\n"));
        assert!(out.contains("\"has\"\"quote\""));
        assert!(out.contains("\"has\nnewline\""));
        // Round-trips.
        assert_eq!(parse(&out).unwrap(), csv);
    }

    #[test]
    fn to_string_with_bad_delim_falls_back_to_comma() {
        let csv = Csv::from_rows(vec![vec!["a".to_string(), "b".to_string()]]);
        // A bad delimiter must not panic; falls back to comma.
        assert_eq!(to_string_with(&csv, '"'), "a,b\n");
    }
}
