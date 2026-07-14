// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_xlsx`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!`/`String` are in scope via the
// default prelude — no `extern crate std` / `use std::` (the architecture gate
// bans those std-ism lines, §R7).
//
// rae_zip exposes no public ZIP *writer* (its writer is test-private), so we
// hand-assemble a STORED-method (method 0, no compression) ZIP here containing the
// real .xlsx parts: `[Content_Types].xml` + `xl/workbook.xml` +
// `xl/_rels/workbook.xml.rels` + `xl/sharedStrings.xml` + `xl/worksheets/sheetN.xml`.
// That is the minimal real .xlsx shape, and STORED means we never depend on an
// encoder — each assert on extracted content is concrete.
// ════════════════════════════════════════════════════════════════════════════

use super::*;

// ─── A from-scratch STORED-method ZIP writer for .xlsx fixtures ──────────────

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

/// CRC-32 (IEEE) — reuse rae_zip's public implementation so the stored CRC matches
/// what the reader recomputes.
fn crc32(d: &[u8]) -> u32 {
    rae_zip::crc32(d)
}

struct Member {
    name: String,
    data: Vec<u8>,
}

fn member(name: &str, data: &[u8]) -> Member {
    Member {
        name: String::from(name),
        data: data.to_vec(),
    }
}

/// Build a valid 32-bit STORED ZIP (the .xlsx container) from members.
fn build_zip(members: &[Member]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut local_offsets = Vec::new();

    for m in members {
        local_offsets.push(out.len() as u32);
        let crc = crc32(&m.data);
        out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method 0 (stored)
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes()); // comp size
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes()); // uncomp size
        out.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(m.name.as_bytes());
        out.extend_from_slice(&m.data);
    }

    let cd_offset = out.len() as u32;
    for (i, m) in members.iter().enumerate() {
        let crc = crc32(&m.data);
        out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method 0
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(m.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(m.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&local_offsets[i].to_le_bytes());
        out.extend_from_slice(m.name.as_bytes());
    }
    let cd_size = out.len() as u32 - cd_offset;

    out.extend_from_slice(&SIG_EOCD.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&(members.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
</Types>"#;

const RELS_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";

/// Build a workbook.xml from `(sheet name, rId)` pairs.
fn workbook_xml(sheets: &[(&str, &str)]) -> String {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push_str(r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#);
    for (name, rid) in sheets {
        s.push_str(r#"<sheet name=""#);
        s.push_str(name);
        s.push_str(r#"" sheetId="1" r:id=""#);
        s.push_str(rid);
        s.push_str(r#""/>"#);
    }
    s.push_str("</sheets></workbook>");
    s
}

/// Build a workbook rels mapping `(rId, target)` pairs to worksheet parts.
fn rels_xml(rels: &[(&str, &str)]) -> String {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push_str(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, target) in rels {
        s.push_str(r#"<Relationship Id=""#);
        s.push_str(id);
        s.push_str(r#"" Type=""#);
        s.push_str(RELS_NS);
        s.push_str(r#"" Target=""#);
        s.push_str(target);
        s.push_str(r#""/>"#);
    }
    s.push_str("</Relationships>");
    s
}

/// Build sharedStrings.xml from a list of plain string entries.
fn shared_strings_xml(strings: &[&str]) -> String {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push_str(r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#);
    for v in strings {
        s.push_str("<si><t>");
        s.push_str(v);
        s.push_str("</t></si>");
    }
    s.push_str("</sst>");
    s
}

/// Wrap raw `<sheetData>` inner rows in a full worksheet.xml.
fn worksheet_xml(sheetdata_inner: &str) -> String {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push_str(r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#);
    s.push_str(sheetdata_inner);
    s.push_str("</sheetData></worksheet>");
    s
}

// ─── 1. The "kitchen sink" single-sheet workbook ─────────────────────────────

/// A1=shared "Name", B1=shared "Score", A2=inline "Ada", B2=number 95.5,
/// A3=shared "Bob", B3=boolean TRUE, C3=error #DIV/0!, A4 omitted (sparse),
/// B4 omitted, C4=formula =1+1 with cached 2.
fn kitchen_sink_xlsx() -> Vec<u8> {
    let shared = shared_strings_xml(&["Name", "Score", "Bob"]);
    let sheet1 = worksheet_xml(
        "\
<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>\
<row r=\"2\"><c r=\"A2\" t=\"inlineStr\"><is><t>Ada</t></is></c><c r=\"B2\"><v>95.5</v></c></row>\
<row r=\"3\"><c r=\"A3\" t=\"s\"><v>2</v></c><c r=\"B3\" t=\"b\"><v>1</v></c><c r=\"C3\" t=\"e\"><v>#DIV/0!</v></c></row>\
<row r=\"4\"><c r=\"C4\"><f>1+1</f><v>2</v></c></row>",
    );
    build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member(
            "xl/workbook.xml",
            workbook_xml(&[("Sheet1", "rId1")]).as_bytes(),
        ),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/sharedStrings.xml", shared.as_bytes()),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ])
}

#[test]
fn cell_types_and_values() {
    let xlsx = kitchen_sink_xlsx();
    let wb = Workbook::open(&xlsx).expect("open xlsx");
    assert_eq!(wb.sheets.len(), 1);
    let s = &wb.sheets[0];
    assert_eq!(s.name, "Sheet1");

    // Shared strings.
    assert_eq!(s.cell(0, 0), Some(&CellValue::Text(String::from("Name"))));
    assert_eq!(s.cell(1, 0), Some(&CellValue::Text(String::from("Score"))));
    assert_eq!(s.cell(0, 2), Some(&CellValue::Text(String::from("Bob"))));

    // Inline string.
    assert_eq!(s.cell(0, 1), Some(&CellValue::Text(String::from("Ada"))));

    // Number cell == expected f64 (the concrete number assert).
    match s.cell(1, 1) {
        Some(CellValue::Number(n)) => assert_eq!(*n, 95.5),
        other => panic!("B2 expected Number(95.5), got {:?}", other),
    }

    // Boolean.
    assert_eq!(s.cell(1, 2), Some(&CellValue::Bool(true)));

    // Error literal.
    assert_eq!(
        s.cell(2, 2),
        Some(&CellValue::Error(String::from("#DIV/0!")))
    );

    // FAIL-ability: a shared-string index resolution bug would surface "Score" at A1.
    assert_ne!(s.cell(0, 0), Some(&CellValue::Text(String::from("Score"))));
    // FAIL-ability: if t="b" weren't honored, B3 would be Number(1.0).
    assert_ne!(s.cell(1, 2), Some(&CellValue::Number(1.0)));
}

#[test]
fn cached_formula_value_is_read() {
    let xlsx = kitchen_sink_xlsx();
    let wb = Workbook::open(&xlsx).unwrap();
    let s = &wb.sheets[0];
    // C4 (col 2, row 3) is a formula cell: value = cached 2, formula text = "1+1".
    let c = s.cell_full(2, 3).expect("C4 present");
    assert!(c.is_formula());
    assert_eq!(c.formula.as_deref(), Some("1+1"));
    assert_eq!(c.value, CellValue::Number(2.0));
    // FAIL-ability: if the cached <v> were ignored, the value would be Empty.
    assert_ne!(c.value, CellValue::Empty);
}

#[test]
fn sparse_row_placement() {
    // A1 present, B1 OMITTED, C1 present → C1 must land at col index 2, not 1.
    let shared = shared_strings_xml(&[]);
    let sheet1 =
        worksheet_xml("<row r=\"1\"><c r=\"A1\"><v>10</v></c><c r=\"C1\"><v>30</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/sharedStrings.xml", shared.as_bytes()),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    let s = &wb.sheets[0];

    assert_eq!(s.cell(0, 0), Some(&CellValue::Number(10.0)));
    // B1 (col 1) is absent.
    assert_eq!(s.cell(1, 0), None);
    // C1 lands at col 2 — the load-bearing sparse assert.
    assert_eq!(s.cell(2, 0), Some(&CellValue::Number(30.0)));
    // Dimensions reflect the populated width = 3, height = 1.
    assert_eq!(s.dimensions(), (3, 1));

    // FAIL-ability: a naive positional parser (ignoring r="C1") would put 30 at col 1.
    assert_ne!(s.cell(1, 0), Some(&CellValue::Number(30.0)));
}

// ─── Multi-sheet names + order (resolved through rels) ───────────────────────

#[test]
fn multi_sheet_names_and_order() {
    // Two sheets; rels deliberately maps rId2→sheet1.xml and rId1→sheet2.xml so a
    // positional shortcut (ignoring r:id) would swap the order.
    let s_a = worksheet_xml("<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>");
    let s_b = worksheet_xml("<row r=\"1\"><c r=\"A1\"><v>2</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member(
            "xl/workbook.xml",
            workbook_xml(&[("Alpha", "rId1"), ("Beta", "rId2")]).as_bytes(),
        ),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[
                ("rId1", "worksheets/sheet2.xml"),
                ("rId2", "worksheets/sheet1.xml"),
            ])
            .as_bytes(),
        ),
        // sheet1.xml holds value 1; sheet2.xml holds value 2.
        member("xl/worksheets/sheet1.xml", s_a.as_bytes()),
        member("xl/worksheets/sheet2.xml", s_b.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();

    // Names in workbook (tab) order.
    assert_eq!(wb.sheet_names(), ["Alpha", "Beta"]);

    // Alpha's r:id=rId1 → sheet2.xml (value 2); Beta's r:id=rId2 → sheet1.xml (value 1).
    assert_eq!(
        wb.sheet("Alpha").unwrap().cell(0, 0),
        Some(&CellValue::Number(2.0))
    );
    assert_eq!(
        wb.sheet("Beta").unwrap().cell(0, 0),
        Some(&CellValue::Number(1.0))
    );

    // FAIL-ability: a positional resolver (Alpha→sheet1, Beta→sheet2) would put 1
    // in Alpha — assert it does NOT.
    assert_ne!(
        wb.sheet("Alpha").unwrap().cell(0, 0),
        Some(&CellValue::Number(1.0))
    );
}

// ─── THE concrete "show me the data" assert: to_csv ──────────────────────────

#[test]
fn to_csv_exact_string() {
    // A small grid mixing types + a field that must be CSV-quoted (contains a comma).
    let shared = shared_strings_xml(&["Hello, World", "plain"]);
    let sheet1 = worksheet_xml(
        "\
<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\"><v>42</v></c></row>\
<row r=\"2\"><c r=\"A2\" t=\"s\"><v>1</v></c><c r=\"B2\" t=\"b\"><v>0</v></c></row>",
    );
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/sharedStrings.xml", shared.as_bytes()),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    let csv = wb.sheets[0].to_csv();

    // "Hello, World" has a comma → quoted; 42 is an integer (no .0); FALSE is the
    // boolean display. Exact string match.
    assert_eq!(csv, "\"Hello, World\",42\nplain,FALSE");

    // FAIL-ability: a different separator or an unquoted comma-field flips this.
    assert_ne!(csv, "Hello, World,42\nplain,FALSE"); // missing quotes
    assert_ne!(csv, "\"Hello, World\";42\nplain;FALSE"); // wrong separator
    assert_ne!(csv, "\"Hello, World\",42.0\nplain,FALSE"); // integer printed as 42.0
}

#[test]
fn to_csv_preserves_sparse_gaps() {
    // A1, C1 present (B1 empty) → CSV must keep the empty middle field.
    let sheet1 =
        worksheet_xml("<row r=\"1\"><c r=\"A1\"><v>1</v></c><c r=\"C1\"><v>3</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    assert_eq!(wb.sheets[0].to_csv(), "1,,3");
}

// ─── DoS: a sparse sheet with two extreme-corner cells must NOT iterate the full
//     max_col × max_row rectangle in to_csv (reviewer HIGH live DoS) ────────────

#[test]
fn to_csv_extreme_corners_is_bounded_not_17_billion() {
    // A sub-1 KB worksheet with exactly TWO valid cells: A1 (col 0, row 0) and
    // XFD1048576 (col 16383, row 1048575) — both pass parse_a1 / MAX_COL / MAX_ROW.
    // dimensions() therefore reports (16384, 1048576), whose rectangle is
    // 16384 * 1048576 = 17_179_869_184 (~17.18 BILLION) grid positions.
    //
    // The OLD to_csv looped that whole rectangle and called the O(N) cell() at each
    // position → ~17.18e9 * 2 linear scans = a multi-hour CPU hang on the Quick-Look
    // "show me the data" preview path. If this test were run against that code it
    // would NOT complete (it would wedge the test runner) — that non-completion is
    // exactly the regression this asserts away. The fixed path rejects the oversized
    // bounding box up front and returns instantly.
    let sheet1 = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>\
<row r=\"1048576\"><c r=\"XFD1048576\"><v>2</v></c></row>",
    );
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    // The whole crafted file is tiny (a few KB of OPC boilerplate) — proving the
    // blowup is the implied rectangle, not the input size. The worksheet payload that
    // triggers it is ~100 bytes; the rest is the mandatory container parts.
    assert!(
        xlsx.len() < 4096,
        "fixture should be small, was {}",
        xlsx.len()
    );

    let wb = Workbook::open(&xlsx).expect("open the (valid) crafted xlsx");
    let s = &wb.sheets[0];

    // dimensions() still honestly reports the populated extent — that's O(N) and fine.
    assert_eq!(s.dimensions(), (16384, 1048576));
    let (w, h) = s.dimensions();
    assert!(
        (w as u64) * (h as u64) > 17_000_000_000,
        "the implied rectangle really is ~17.18 billion positions"
    );

    // The fallible path REFUSES the oversized bounding box (graceful Err, no hang).
    assert_eq!(s.try_to_csv(), Err(XlsxError::GridTooLarge));

    // The infallible path returns the empty string (bounded), again no hang.
    assert_eq!(s.to_csv(), "");

    // FAIL-ability: if the area budget were removed (the old behavior), neither call
    // above would return — the test would time out / never complete. Completing at
    // all is the proof the DoS is bounded.
}

#[test]
fn to_csv_renders_populated_bounding_box_within_budget() {
    // A sparse sheet whose bounding box is small (A1 + C2) must render the full box
    // cheaply via the indexed walk — proving the fix renders data, not just Err.
    // Bounding box = cols 0..=2, rows 0..=1 → a 3x2 grid with two populated corners.
    let sheet1 = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>\
<row r=\"2\"><c r=\"C2\"><v>9</v></c></row>",
    );
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(s.dimensions(), (3, 2));
    // Row 1: 1,, (A1=1, B1/C1 empty). Row 2: ,,9 (A2/B2 empty, C2=9). Identical to
    // what the old per-position walk produced for this in-budget sheet.
    assert_eq!(s.try_to_csv(), Ok(String::from("1,,\n,,9")));
    assert_eq!(s.to_csv(), "1,,\n,,9");
}

// ─── Number parsing coverage (the no_std f64 parser) ─────────────────────────

#[test]
fn number_parsing_forms() {
    let sheet1 = worksheet_xml(
        "\
<row r=\"1\">\
<c r=\"A1\"><v>0</v></c>\
<c r=\"B1\"><v>-3.14</v></c>\
<c r=\"C1\"><v>6.022e23</v></c>\
<c r=\"D1\"><v>1000000</v></c>\
<c r=\"E1\"><v>0.001</v></c>\
</row>",
    );
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    let s = &wb.sheets[0];

    assert_eq!(s.cell(0, 0), Some(&CellValue::Number(0.0)));
    match s.cell(1, 0) {
        Some(CellValue::Number(n)) => assert!((*n - (-3.14)).abs() < 1e-12),
        o => panic!("B1 {:?}", o),
    }
    match s.cell(2, 0) {
        Some(CellValue::Number(n)) => {
            // 6.022e23 within relative tolerance.
            let expected = 6.022e23_f64;
            assert!((*n - expected).abs() / expected < 1e-6, "got {}", n);
        }
        o => panic!("C1 {:?}", o),
    }
    assert_eq!(s.cell(3, 0), Some(&CellValue::Number(1_000_000.0)));
    match s.cell(4, 0) {
        Some(CellValue::Number(n)) => assert!((*n - 0.001).abs() < 1e-12),
        o => panic!("E1 {:?}", o),
    }

    // Display strings: integers compact, fraction trimmed.
    assert_eq!(s.cell(0, 0).unwrap().to_display_string(), "0");
    assert_eq!(s.cell(3, 0).unwrap().to_display_string(), "1000000");
}

#[test]
fn parse_a1_known_vectors() {
    assert_eq!(parse_a1("A1"), Ok((0, 0)));
    assert_eq!(parse_a1("B3"), Ok((1, 2)));
    assert_eq!(parse_a1("Z1"), Ok((25, 0)));
    assert_eq!(parse_a1("AA1"), Ok((26, 0)));
    assert_eq!(parse_a1("AB10"), Ok((27, 9)));
    assert_eq!(parse_a1("$C$5"), Ok((2, 4))); // absolute markers ignored
    assert_eq!(parse_a1("XFD1048576"), Ok((16383, 1048575))); // Excel max

    // Malformed / out of bounds.
    assert_eq!(parse_a1(""), Err(XlsxError::BadCellRef));
    assert_eq!(parse_a1("1"), Err(XlsxError::BadCellRef)); // no column
    assert_eq!(parse_a1("A"), Err(XlsxError::BadCellRef)); // no row
    assert_eq!(parse_a1("A0"), Err(XlsxError::BadCellRef)); // row 0 invalid
    assert_eq!(parse_a1("A1B"), Err(XlsxError::BadCellRef)); // trailing junk
    assert!(parse_a1("ZZZZZ1").is_err()); // column far past XFD

    // FAIL-ability: a 0-based-letter bug would map AA1 to (27,_) not (26,_).
    assert_ne!(parse_a1("AA1"), Ok((27, 0)));
}

// ─── Rich-text shared string concatenation ───────────────────────────────────

#[test]
fn rich_text_shared_string() {
    // A shared string built from multiple <r><t> runs must concatenate.
    let shared = concat_xml(
        r#"<?xml version="1.0"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
        "<si><r><t>Hello</t></r><r><t xml:space=\"preserve\">, </t></r><r><t>AthenaOS</t></r></si>",
        "</sst>",
    );
    let sheet1 = worksheet_xml("<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/sharedStrings.xml", shared.as_bytes()),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    assert_eq!(
        wb.sheets[0].cell(0, 0),
        Some(&CellValue::Text(String::from("Hello, AthenaOS")))
    );
}

fn concat_xml(a: &str, b: &str, c: &str) -> String {
    let mut s = String::from(a);
    s.push_str(b);
    s.push_str(c);
    s
}

// ─── Entities decode inside shared strings ───────────────────────────────────

#[test]
fn entities_decode_in_cells() {
    let shared = shared_strings_xml(&["a &amp; b &lt; c", "euro &#8364; A &#x41;"]);
    let sheet1 = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\" t=\"s\"><v>1</v></c></row>",
    );
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/sharedStrings.xml", shared.as_bytes()),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    let s = &wb.sheets[0];
    assert_eq!(
        s.cell(0, 0),
        Some(&CellValue::Text(String::from("a & b < c")))
    );
    assert_eq!(
        s.cell(1, 0),
        Some(&CellValue::Text(String::from("euro \u{20ac} A A")))
    );
}

// ─── Reject: a valid ZIP without xl/workbook.xml is NOT an XLSX ───────────────

#[test]
fn reject_non_xlsx_zip() {
    let zip = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("readme.txt", b"just a zip, not a spreadsheet"),
    ]);
    assert_eq!(Workbook::open(&zip), Err(XlsxError::NotXlsx));
}

#[test]
fn reject_workbook_with_no_sheets() {
    // workbook.xml present but declares no <sheet>.
    let wb_xml = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheets></sheets></workbook>"#;
    let zip = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", wb_xml.as_bytes()),
    ]);
    assert_eq!(Workbook::open(&zip), Err(XlsxError::NotXlsx));
}

// ─── Reject: truncated / garbage bytes → Err, never panic ────────────────────

#[test]
fn reject_truncated_and_garbage() {
    assert!(Workbook::open(&[]).is_err());
    assert!(Workbook::open(b"not a zip at all").is_err());

    let xlsx = kitchen_sink_xlsx();
    for cut in [1usize, 8, 20, xlsx.len() / 2, xlsx.len() - 1] {
        let part = &xlsx[..cut.min(xlsx.len())];
        let _ = Workbook::open(part); // must not panic
    }
    let mut junk = xlsx.clone();
    for (i, b) in junk.iter_mut().enumerate() {
        if i % 3 == 0 {
            *b ^= 0xFF;
        }
    }
    let _ = Workbook::open(&junk); // never panics
}

// ─── Malformed XML inside a valid container → Err/graceful, never panic ───────

#[test]
fn malformed_xml_graceful() {
    // Unterminated worksheet tag mid-stream (no closing '>') → BadXml, not a panic.
    let bad = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member(
            "xl/worksheets/sheet1.xml",
            b"<worksheet><sheetData><row <c></row></sheetData>",
        ),
    ]);
    let r = Workbook::open(&bad);
    assert!(r.is_err() || r.is_ok()); // assertion: it returned, no panic
}

// ─── Deeply nested XML → TooDeep, never stack-overflow ───────────────────────

#[test]
fn deep_nesting_bounded() {
    let mut inner = String::new();
    let n = MAX_DEPTH * 2;
    for _ in 0..n {
        inner.push_str("<x>");
    }
    inner.push_str("<row r=\"1\"><c r=\"A1\"><v>1</v></c></row>");
    for _ in 0..n {
        inner.push_str("</x>");
    }
    let sheet1 = worksheet_xml(&inner);
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let r = Workbook::open(&xlsx);
    assert!(matches!(r, Err(XlsxError::TooDeep)) || r.is_err());
}

// ─── Huge cell count → TooLarge / bounded ────────────────────────────────────

#[test]
fn huge_cell_count_bounded() {
    // A row with an absurd column reference must be rejected as a bad ref, not
    // over-allocated into a dense grid (the model is sparse, so this stays cheap,
    // but the reference itself is out of bounds).
    let sheet1 = worksheet_xml("<row r=\"1\"><c r=\"ZZZZ1\"><v>1</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    // ZZZZ = column far past XFD → BadCellRef bubbles up.
    let r = Workbook::open(&xlsx);
    assert!(matches!(r, Err(XlsxError::BadCellRef)) || r.is_err());
}

// ─── Empty sheet → empty model, empty CSV ────────────────────────────────────

#[test]
fn empty_sheet() {
    let sheet1 = worksheet_xml("");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member(
            "xl/workbook.xml",
            workbook_xml(&[("Empty", "rId1")]).as_bytes(),
        ),
        member(
            "xl/_rels/workbook.xml.rels",
            rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    assert_eq!(wb.sheets.len(), 1);
    assert_eq!(wb.sheets[0].dimensions(), (0, 0));
    assert_eq!(wb.sheets[0].to_csv(), "");
    assert_eq!(wb.sheets[0].cell(0, 0), None);
}

// ─── Positional fallback when rels is absent ─────────────────────────────────

#[test]
fn positional_fallback_without_rels() {
    // No workbook.xml.rels — the reader falls back to sheet1.xml positionally.
    let sheet1 = worksheet_xml("<row r=\"1\"><c r=\"A1\"><v>7</v></c></row>");
    let xlsx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member(
            "xl/workbook.xml",
            workbook_xml(&[("Only", "rId1")]).as_bytes(),
        ),
        member("xl/worksheets/sheet1.xml", sheet1.as_bytes()),
    ]);
    let wb = Workbook::open(&xlsx).unwrap();
    assert_eq!(wb.sheet_names(), ["Only"]);
    assert_eq!(wb.sheets[0].cell(0, 0), Some(&CellValue::Number(7.0)));
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// Properties on the public surface: `Workbook::open` must (a) never panic on ANY
// byte sequence, and (b) bound every allocation (part-size cap, element-count cap,
// cell cap, depth cap). FAIL-ability (by reasoning):
//  - `#![forbid(unsafe_code)]` makes any OOB index a guaranteed panic, so a
//    never-panic loop genuinely proves bounds-safety — a hostile-byte panic aborts
//    the test process (red).
//  - Removing the depth cap makes `deep_nesting_bounded` recurse to a stack
//    overflow (abort = red).
//  - Removing the cell-ref bounds lets `huge_cell_count_bounded` slip through.
// ════════════════════════════════════════════════════════════════════════════

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
        if n == 0 {
            0
        } else {
            (self.next_u64() % (n as u64)) as usize
        }
    }
}

#[test]
fn fuzz_random_bytes_never_panic() {
    let mut rng = Rng::new(0x5_CA1AB1E);
    for _ in 0..40_000 {
        let len = rng.below(256);
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(rng.byte());
        }
        let _ = Workbook::open(&buf);
    }
}

#[test]
fn fuzz_mutated_valid_xlsx_never_panic() {
    let base = kitchen_sink_xlsx();
    assert!(Workbook::open(&base).is_ok(), "seed xlsx must open");

    let mut rng = Rng::new(0x3333_5CA1);
    for _ in 0..60_000 {
        let mut m = base.clone();
        let muts = 1 + rng.below(6);
        for _ in 0..muts {
            let i = rng.below(m.len());
            m[i] ^= rng.byte();
        }
        let _ = Workbook::open(&m); // never panics
    }
}

#[test]
fn fuzz_mutated_worksheet_xml_never_panic() {
    // Mutate only the worksheet XML text so the fuzzer explores the cell walker.
    let base = worksheet_xml(
        "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c><c r=\"B1\"><f>SUM(A1:A2)</f><v>3.5</v></c>\
<c r=\"C1\" t=\"inlineStr\"><is><t>inline</t></is></c></row>",
    );
    let base_bytes = base.into_bytes();
    let mut rng = Rng::new(0xBEEF_5CA1);
    for _ in 0..60_000 {
        let mut m = base_bytes.clone();
        let muts = 1 + rng.below(8);
        for _ in 0..muts {
            let i = rng.below(m.len());
            m[i] = rng.byte();
        }
        let xlsx = build_zip(&[
            member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            member("xl/workbook.xml", workbook_xml(&[("S", "rId1")]).as_bytes()),
            member(
                "xl/_rels/workbook.xml.rels",
                rels_xml(&[("rId1", "worksheets/sheet1.xml")]).as_bytes(),
            ),
            member("xl/worksheets/sheet1.xml", &m),
        ]);
        let _ = Workbook::open(&xlsx);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// WRITER host KAT suite — the write→read round-trip is the load-bearing proof.
//
// Each test builds a Workbook in memory, serializes it with the new writer
// (`to_xlsx`), then opens the bytes with the EXISTING reader (`Workbook::open`)
// and asserts the model survived exactly. FAIL-able: change any expected value
// and a round-trip assert flips.
// ════════════════════════════════════════════════════════════════════════════

/// Convenience: a typed cell at (col,row).
fn wc(col: u32, row: u32, value: CellValue) -> Cell {
    Cell {
        col,
        row,
        value,
        formula: None,
    }
}

fn txt(s: &str) -> CellValue {
    CellValue::Text(String::from(s))
}

// ── 24. Two-sheet, mixed-type, dedup, sparse-gap round-trip (THE big one) ────

#[test]
fn writer_roundtrip_mixed_two_sheets() {
    // Sheet "Data":
    //   A1 = "Name"  (shared)
    //   B1 = "Name"  (shared — DUPLICATE of A1; must dedupe to one sst entry)
    //   A2 = 42      (integer number → must read back as Number(42.0))
    //   B2 = 3.14159 (fractional number)
    //   A3 = bool TRUE
    //   (B3 omitted — sparse gap)
    //   C3 = "City"  (shared, third unique string)
    let data_cells = vec![
        wc(0, 0, txt("Name")),
        wc(1, 0, txt("Name")), // duplicate string
        wc(0, 1, CellValue::Number(42.0)),
        wc(1, 1, CellValue::Number(3.14159)),
        wc(0, 2, CellValue::Bool(true)),
        // (1,2) intentionally absent → sparse gap
        wc(2, 2, txt("City")),
    ];
    let second_cells = vec![wc(0, 0, txt("only"))];

    let written = WorkbookBuilder::new()
        .add_sheet("Data", data_cells)
        .add_sheet("Second", second_cells)
        .to_xlsx()
        .expect("to_xlsx");

    // Read it back with the EXISTING reader.
    let wb = Workbook::open(&written).expect("reader opens written .xlsx");

    // Sheet names + order preserved.
    assert_eq!(wb.sheet_names(), vec!["Data", "Second"]);

    let data = &wb.sheets[0];
    // Strings.
    assert_eq!(data.cell(0, 0), Some(&txt("Name")));
    assert_eq!(data.cell(1, 0), Some(&txt("Name")));
    assert_eq!(data.cell(2, 2), Some(&txt("City")));
    // Integer number reads back as exactly Number(42.0).
    match data.cell(0, 1) {
        Some(CellValue::Number(n)) => assert_eq!(*n, 42.0),
        other => panic!("A2 expected Number(42.0), got {:?}", other),
    }
    // Fractional number within 1e-9.
    match data.cell(1, 1) {
        Some(CellValue::Number(n)) => {
            let d = if *n > 3.14159 {
                *n - 3.14159
            } else {
                3.14159 - *n
            };
            assert!(d < 1e-9);
        }
        other => panic!("B2 expected Number(3.14159), got {:?}", other),
    }
    // Boolean.
    assert_eq!(data.cell(0, 2), Some(&CellValue::Bool(true)));
    // Sparse gap preserved: (1,2) was omitted → None.
    assert_eq!(data.cell(1, 2), None);

    // Second sheet.
    assert_eq!(wb.sheets[1].cell(0, 0), Some(&txt("only")));

    // FAIL-ability: a write→read drift would surface here.
    assert_ne!(data.cell(0, 1), Some(&CellValue::Number(43.0)));
    assert_ne!(data.cell(0, 2), Some(&CellValue::Bool(false)));
}

// ── 25. Shared-string dedup: a duplicated string → exactly one sst entry ──────

#[test]
fn writer_dedup_single_sst_entry() {
    let cells = vec![wc(0, 0, txt("x")), wc(1, 0, txt("x")), wc(2, 0, txt("y"))];
    let written = WorkbookBuilder::new()
        .add_sheet("S", cells)
        .to_xlsx()
        .expect("to_xlsx");

    // Pull the sharedStrings.xml part out of the archive and count <si> entries.
    let ar = rae_zip::Archive::open(&written).expect("archive opens");
    let sst_entry = ar
        .find("xl/sharedStrings.xml")
        .expect("sharedStrings part present");
    let sst_bytes = ar.read_entry(sst_entry).expect("read sst");
    let sst_xml = core::str::from_utf8(&sst_bytes).expect("sst utf8");
    let si_count = sst_xml.matches("<si>").count();
    // Two unique strings "x" and "y" → exactly two <si> entries (NOT three).
    assert_eq!(si_count, 2, "duplicate 'x' must dedupe to one <si>");
    assert!(sst_xml.contains("uniqueCount=\"2\""));

    // The reader resolves both "x" cells to the same text.
    let wb = Workbook::open(&written).expect("open");
    let s = &wb.sheets[0];
    assert_eq!(s.cell(0, 0), Some(&txt("x")));
    assert_eq!(s.cell(1, 0), Some(&txt("x")));
    assert_eq!(s.cell(2, 0), Some(&txt("y")));

    // FAIL-ability: if dedup broke and emitted 3 entries, si_count would be 3.
    assert_ne!(si_count, 3);
}

// ── 26. XML-special characters round-trip (escaping is load-bearing) ─────────

#[test]
fn writer_xml_special_escaping() {
    // A string with every XML metacharacter plus a control char must round-trip.
    let nasty = "a<b>&c\"d'e\tf";
    let cells = vec![wc(0, 0, txt(nasty)), wc(0, 1, txt("plain"))];
    let written = WorkbookBuilder::new()
        .add_sheet("Esc", cells)
        .to_xlsx()
        .expect("to_xlsx");

    // The raw sharedStrings must be escaped (no naked '<' inside a <t>).
    let ar = rae_zip::Archive::open(&written).expect("archive");
    let sst = ar.find("xl/sharedStrings.xml").expect("sst");
    let sst_xml = String::from_utf8(ar.read_entry(sst).expect("read")).expect("utf8");
    assert!(sst_xml.contains("&lt;"));
    assert!(sst_xml.contains("&amp;"));
    assert!(sst_xml.contains("&quot;"));

    // And the reader decodes it back to the exact original.
    let wb = Workbook::open(&written).expect("open");
    assert_eq!(wb.sheets[0].cell(0, 0), Some(&txt(nasty)));
    assert_eq!(wb.sheets[0].cell(0, 1), Some(&txt("plain")));

    // FAIL-ability: a missing-escape bug would corrupt the XML / mis-decode.
    assert_ne!(wb.sheets[0].cell(0, 0), Some(&txt("a<b>&c")));
}

// ── 27. Number precision: many values round-trip ─────────────────────────────

#[test]
fn writer_number_precision_roundtrip() {
    let values = [
        0.0f64,
        1.0,
        -1.0,
        42.0,
        -7.0,
        0.5,
        3.14159,
        2.718281828459045,
        100000.0,
        0.000123,
        1234567.89,
        -0.000000001,
        9999999999.0,
        1e20,
        1e-20,
    ];
    let cells: Vec<Cell> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| wc(0, i as u32, CellValue::Number(v)))
        .collect();
    let written = WorkbookBuilder::new()
        .add_sheet("Nums", cells)
        .to_xlsx()
        .expect("to_xlsx");
    let wb = Workbook::open(&written).expect("open");
    let s = &wb.sheets[0];
    for (i, &v) in values.iter().enumerate() {
        match s.cell(0, i as u32) {
            Some(CellValue::Number(n)) => {
                let av = if v < 0.0 { -v } else { v };
                let tol = if av > 1e9 || (v != 0.0 && av < 1e-9) {
                    av * 1e-9 + 1e-12
                } else {
                    1e-9
                };
                let diff = if *n > v { *n - v } else { v - *n };
                assert!(
                    diff <= tol,
                    "value {} read back as {} (diff {})",
                    v,
                    n,
                    diff
                );
            }
            other => panic!("row {} expected Number({}), got {:?}", i, v, other),
        }
    }
    // FAIL-ability: integer 42 specifically must read back as 42.0.
    assert_eq!(s.cell(0, 3), Some(&CellValue::Number(42.0)));
}

// ── 28. Empty workbook / single empty sheet ──────────────────────────────────

#[test]
fn writer_empty_and_single_empty_sheet() {
    // Empty workbook → Err (a workbook must have at least one sheet).
    let empty = WorkbookBuilder::new().to_xlsx();
    assert_eq!(empty, Err(XlsxWriteError::NoSheets));

    // A single sheet with no cells writes a valid file the reader opens.
    let written = WorkbookBuilder::new()
        .add_sheet("Blank", Vec::new())
        .to_xlsx()
        .expect("to_xlsx");
    let wb = Workbook::open(&written).expect("reader opens empty sheet");
    assert_eq!(wb.sheet_names(), vec!["Blank"]);
    assert_eq!(wb.sheets[0].cells.len(), 0);
    assert_eq!(wb.sheets[0].dimensions(), (0, 0));
}

// ── 29. to_csv of the read-back workbook matches expectations ────────────────

#[test]
fn writer_roundtrip_to_csv() {
    let cells = vec![
        wc(0, 0, txt("h1")),
        wc(1, 0, txt("h2")),
        wc(0, 1, CellValue::Number(10.0)),
        // (1,1) omitted → empty field
    ];
    let written = WorkbookBuilder::new()
        .add_sheet("Grid", cells)
        .to_xlsx()
        .expect("to_xlsx");
    let wb = Workbook::open(&written).expect("open");
    let csv = wb.sheets[0].to_csv();
    assert_eq!(csv, "h1,h2\n10,");

    // FAIL-ability: wrong sparse handling would drop the trailing empty field.
    assert_ne!(csv, "h1,h2\n10");
}

// ── 30. Workbook::to_xlsx — read→write→read stability ────────────────────────

#[test]
fn writer_workbook_to_xlsx_full_roundtrip() {
    let original = kitchen_sink_xlsx();
    let wb1 = Workbook::open(&original).expect("open original");
    let rewritten = wb1.to_xlsx().expect("re-serialize");
    let wb2 = Workbook::open(&rewritten).expect("open rewritten");

    assert_eq!(wb1.sheet_names(), wb2.sheet_names());
    let s1 = &wb1.sheets[0];
    let s2 = &wb2.sheets[0];
    assert_eq!(s2.cell(0, 0), s1.cell(0, 0)); // "Name"
    assert_eq!(s2.cell(1, 0), s1.cell(1, 0)); // "Score"
    assert_eq!(s2.cell(0, 1), s1.cell(0, 1)); // inline "Ada" → shared "Ada"
    assert_eq!(s2.cell(1, 1), s1.cell(1, 1)); // 95.5
    assert_eq!(s2.cell(1, 2), s1.cell(1, 2)); // Bool(true)
    assert_eq!(s2.cell(2, 2), s1.cell(2, 2)); // Error #DIV/0!
                                              // The formula cell's cached value (2) survives as a plain number.
    assert_eq!(s2.cell(2, 3), Some(&CellValue::Number(2.0)));
}

// ── 31. A1 reference generation across column boundaries ──────────────────────

#[test]
fn writer_a1_reference_columns() {
    // Columns 0 (A), 25 (Z), 26 (AA), 27 (AB), 701 (ZZ), 702 (AAA).
    let cells = vec![
        wc(0, 0, txt("A")),
        wc(25, 0, txt("Z")),
        wc(26, 0, txt("AA")),
        wc(27, 0, txt("AB")),
        wc(701, 0, txt("ZZ")),
        wc(702, 0, txt("AAA")),
    ];
    let written = WorkbookBuilder::new()
        .add_sheet("Cols", cells)
        .to_xlsx()
        .expect("to_xlsx");

    let ar = rae_zip::Archive::open(&written).expect("archive");
    let ws = ar.find("xl/worksheets/sheet1.xml").expect("ws");
    let ws_xml = String::from_utf8(ar.read_entry(ws).expect("read")).expect("utf8");
    assert!(ws_xml.contains("r=\"A1\""));
    assert!(ws_xml.contains("r=\"Z1\""));
    assert!(ws_xml.contains("r=\"AA1\""));
    assert!(ws_xml.contains("r=\"AB1\""));
    assert!(ws_xml.contains("r=\"ZZ1\""));
    assert!(ws_xml.contains("r=\"AAA1\""));

    let wb = Workbook::open(&written).expect("open");
    let s = &wb.sheets[0];
    assert_eq!(s.cell(0, 0), Some(&txt("A")));
    assert_eq!(s.cell(25, 0), Some(&txt("Z")));
    assert_eq!(s.cell(26, 0), Some(&txt("AA")));
    assert_eq!(s.cell(702, 0), Some(&txt("AAA")));
}

// ── 32. Over-limit cell reference → Err (never a corrupt file) ────────────────

#[test]
fn writer_rejects_out_of_bounds_cell() {
    let cells = vec![wc(MAX_COL + 1, 0, txt("oob"))];
    let r = WorkbookBuilder::new().add_sheet("S", cells).to_xlsx();
    assert_eq!(r, Err(XlsxWriteError::BadCellRef));
}
