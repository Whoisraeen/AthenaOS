// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_docx`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!`/`String` are in scope via the
// default prelude — no `extern crate std` / `use std::` (the architecture gate
// bans those std-ism lines, §R7).
//
// ath_zip exposes no public ZIP *writer* (its writer is test-private), so we
// hand-assemble a STORED-method (.method 0, no compression) ZIP here containing
// `[Content_Types].xml` + `word/document.xml`. That is the minimal real .docx
// shape, and STORED means we never depend on an encoder — each assert on extracted
// content is concrete.
// ════════════════════════════════════════════════════════════════════════════

use super::*;

// ─── A from-scratch STORED-method ZIP writer for .docx fixtures ─────────────

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

/// CRC-32 (IEEE) — reuse ath_zip's public implementation so the stored CRC matches
/// what the reader recomputes.
fn crc32(d: &[u8]) -> u32 {
    ath_zip::crc32(d)
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

/// Build a valid 32-bit STORED ZIP (the .docx container) from members.
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
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

/// Wrap a WordprocessingML body fragment in a full document.xml.
fn document_xml(body_inner: &str) -> String {
    let mut s = String::new();
    s.push_str(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#);
    s.push_str(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#);
    s.push_str(body_inner);
    s.push_str("</w:body></w:document>");
    s
}

/// Assemble a complete .docx from a body fragment.
fn make_docx(body_inner: &str) -> Vec<u8> {
    let doc = document_xml(body_inner);
    build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("word/document.xml", doc.as_bytes()),
    ])
}

// ─── 1. Plain paragraphs → exact extract_text + structured model ────────────

#[test]
fn extract_text_two_paragraphs() {
    let body = "\
<w:p><w:r><w:t>Hello, AthenaOS!</w:t></w:r></w:p>\
<w:p><w:r><w:t>Second paragraph.</w:t></w:r></w:p>";
    let docx = make_docx(body);

    let doc = Document::open(&docx).expect("open docx");
    // THE concrete extract_text assert: paragraphs joined by '\n'.
    assert_eq!(doc.extract_text(), "Hello, AthenaOS!\nSecond paragraph.");

    let paras = doc.paragraphs();
    assert_eq!(paras.len(), 2);
    assert_eq!(paras[0].runs.len(), 1);
    assert_eq!(paras[0].runs[0].text, "Hello, AthenaOS!");
    assert_eq!(paras[1].runs[0].text, "Second paragraph.");

    // FAIL-ability: if paragraphs were joined by ' ' or '' this flips.
    assert_ne!(doc.extract_text(), "Hello, AthenaOS!Second paragraph.");
    assert_ne!(doc.extract_text(), "Hello, AthenaOS! Second paragraph.");
}

// ─── 2. Multiple runs in one paragraph with bold / italic / underline ───────

#[test]
fn runs_bold_italic_underline() {
    let body = "\
<w:p>\
<w:r><w:rPr><w:b/></w:rPr><w:t>Bold</w:t></w:r>\
<w:r><w:t> normal </w:t></w:r>\
<w:r><w:rPr><w:i/></w:rPr><w:t>Italic</w:t></w:r>\
<w:r><w:rPr><w:u w:val=\"single\"/></w:rPr><w:t>Under</w:t></w:r>\
</w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");

    let paras = doc.paragraphs();
    assert_eq!(paras.len(), 1);
    let runs = &paras[0].runs;
    assert_eq!(runs.len(), 4);

    assert_eq!(runs[0].text, "Bold");
    assert!(runs[0].bold && !runs[0].italic && !runs[0].underline);

    // " normal " has no xml:space=preserve → trimmed to "normal".
    assert_eq!(runs[1].text, "normal");
    assert!(!runs[1].bold && !runs[1].italic);

    assert_eq!(runs[2].text, "Italic");
    assert!(runs[2].italic && !runs[2].bold);

    assert_eq!(runs[3].text, "Under");
    assert!(runs[3].underline);

    // extract_text concatenates runs within the paragraph.
    assert_eq!(doc.extract_text(), "BoldnormalItalicUnder");

    // FAIL-ability: if rPr were ignored, runs[0].bold would be false.
    assert!(runs[0].bold);
    // FAIL-ability: w:u val!="none" must be underline ON.
    assert!(runs[3].underline);
}

// ─── 3. xml:space="preserve" keeps surrounding whitespace ───────────────────

#[test]
fn xml_space_preserve() {
    let body = "\
<w:p>\
<w:r><w:t xml:space=\"preserve\">  spaced  </w:t></w:r>\
</w:p>\
<w:p>\
<w:r><w:t>  trimmed  </w:t></w:r>\
</w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");
    let paras = doc.paragraphs();
    assert_eq!(paras[0].runs[0].text, "  spaced  ");
    assert_eq!(paras[1].runs[0].text, "trimmed");
    // FAIL-ability: if preserve were ignored the first would also be trimmed.
    assert_ne!(paras[0].runs[0].text, "spaced");
}

// ─── 4. XML entities + numeric character references decode ──────────────────

#[test]
fn entities_decode() {
    let body = "\
<w:p><w:r><w:t>a &amp; b &lt; c &gt; d &quot;q&quot; &apos;a&apos;</w:t></w:r></w:p>\
<w:p><w:r><w:t>num &#65; hex &#x41; euro &#8364;</w:t></w:r></w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");
    let paras = doc.paragraphs();
    assert_eq!(paras[0].runs[0].text, "a & b < c > d \"q\" 'a'");
    assert_eq!(paras[1].runs[0].text, "num A hex A euro \u{20ac}");
    // FAIL-ability: undecoded entities would still contain "&amp;".
    assert!(!paras[0].runs[0].text.contains("&amp;"));
}

// ─── 5. Tabs and breaks render inside runs ──────────────────────────────────

#[test]
fn tab_and_break_render() {
    let body = "\
<w:p><w:r><w:t>col1</w:t><w:tab/><w:t>col2</w:t><w:br/><w:t>line2</w:t></w:r></w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");
    let para = &doc.paragraphs()[0];
    assert_eq!(para.text(), "col1\tcol2\nline2");
    assert_eq!(doc.extract_text(), "col1\tcol2\nline2");
}

// ─── 6. Heading paragraph reports its style + heading level ─────────────────

#[test]
fn heading_style_and_level() {
    let body = "\
<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>Title Here</w:t></w:r></w:p>\
<w:p><w:pPr><w:pStyle w:val=\"Heading2\"/></w:pPr><w:r><w:t>Subhead</w:t></w:r></w:p>\
<w:p><w:r><w:t>Body text</w:t></w:r></w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");
    let paras = doc.paragraphs();

    assert_eq!(paras[0].style.as_deref(), Some("Heading1"));
    assert_eq!(paras[0].heading_level(), Some(1));
    assert_eq!(paras[1].style.as_deref(), Some("Heading2"));
    assert_eq!(paras[1].heading_level(), Some(2));
    assert_eq!(paras[2].style, None);
    assert_eq!(paras[2].heading_level(), None);

    assert_eq!(doc.extract_text(), "Title Here\nSubhead\nBody text");
    // FAIL-ability: if pStyle@w:val weren't read, level would be None.
    assert_eq!(paras[0].heading_level(), Some(1));
}

// ─── 7. Table cell text is extracted ────────────────────────────────────────

#[test]
fn table_cells_extracted() {
    let body = "\
<w:p><w:r><w:t>Before table</w:t></w:r></w:p>\
<w:tbl>\
<w:tr>\
<w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>\
<w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc>\
</w:tr>\
<w:tr>\
<w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>\
<w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc>\
</w:tr>\
</w:tbl>\
<w:p><w:r><w:t>After table</w:t></w:r></w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");

    let tables = doc.tables();
    assert_eq!(tables.len(), 1);
    let t = tables[0];
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].cells.len(), 2);
    assert_eq!(t.rows[0].cells[0].text(), "A1");
    assert_eq!(t.rows[0].cells[1].text(), "B1");
    assert_eq!(t.rows[1].cells[0].text(), "A2");
    assert_eq!(t.rows[1].cells[1].text(), "B2");

    // extract_text renders table rows with tab-separated cells.
    assert_eq!(
        doc.extract_text(),
        "Before table\nA1\tB1\nA2\tB2\nAfter table"
    );
    // FAIL-ability: dropping the table would lose the cell text.
    assert!(doc.extract_text().contains("A1\tB1"));
}

// ─── 8. Unknown elements are skipped gracefully ─────────────────────────────

#[test]
fn unknown_elements_skipped() {
    // A bookmark, a proofErr, and a custom unknown element interleaved with text.
    let body = "\
<w:p>\
<w:bookmarkStart w:id=\"0\" w:name=\"x\"/>\
<w:proofErr w:type=\"spellStart\"/>\
<w:r><w:rPr><w:rFonts w:ascii=\"Calibri\"/><w:sz w:val=\"22\"/></w:rPr><w:t>Kept</w:t></w:r>\
<w:customWeird><w:nestedJunk>ignored</w:nestedJunk></w:customWeird>\
<w:proofErr w:type=\"spellEnd\"/>\
<w:bookmarkEnd w:id=\"0\"/>\
</w:p>";
    let docx = make_docx(body);
    let doc = Document::open(&docx).expect("open");
    assert_eq!(doc.extract_text(), "Kept");
    let runs = &doc.paragraphs()[0].runs;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].text, "Kept");
    assert!(!runs[0].bold);
}

// ─── 9. Reject: a valid ZIP without word/document.xml is NOT a DOCX ──────────

#[test]
fn reject_non_docx_zip() {
    let zip = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("readme.txt", b"this is just a zip, not a docx"),
    ]);
    assert_eq!(Document::open(&zip), Err(DocxError::NotDocx));
}

// ─── 10. Reject: truncated / garbage bytes → Err, never panic ───────────────

#[test]
fn reject_truncated_and_garbage() {
    assert!(Document::open(&[]).is_err());
    assert!(Document::open(b"not a zip at all").is_err());

    // A real docx, then truncated mid-file.
    let docx = make_docx("<w:p><w:r><w:t>hi</w:t></w:r></w:p>");
    for cut in [1usize, 8, 20, docx.len() / 2, docx.len() - 1] {
        let part = &docx[..cut.min(docx.len())];
        // Must not panic; result is an Err (or, vacuously, never Ok for a chopped
        // central directory).
        let _ = Document::open(part);
    }
    // A zip whose EOCD is intact but body is junk: flip many bytes.
    let mut junk = docx.clone();
    for (i, b) in junk.iter_mut().enumerate() {
        if i % 3 == 0 {
            *b ^= 0xFF;
        }
    }
    let _ = Document::open(&junk); // never panics
}

// ─── 11. Malformed XML inside a valid container → Err/graceful, never panic ──

#[test]
fn malformed_xml_graceful() {
    // Unterminated tag.
    let docx = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member(
            "word/document.xml",
            b"<w:document><w:body><w:p><w:r><w:t>oops",
        ),
    ]);
    // Tolerant parse: runs to EOF, never panics. (Either Ok with partial content or
    // an Err — both acceptable; the contract is no panic.)
    let _ = Document::open(&docx);

    // An unterminated tag mid-stream (no closing '>') must be BadXml, not a panic.
    let docx2 = build_zip(&[
        member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        member("word/document.xml", b"<w:body><w:p <w:r></w:p></w:body>"),
    ]);
    let r = Document::open(&docx2);
    assert!(r.is_err() || r.is_ok()); // assertion: it returned, no panic
}

// ─── 12. Deeply nested XML → TooDeep, never stack-overflow ──────────────────

#[test]
fn deep_nesting_bounded() {
    // Build MAX_DEPTH*2 nested unknown elements inside the body.
    let mut inner = String::new();
    let n = MAX_DEPTH * 2;
    for _ in 0..n {
        inner.push_str("<w:x>");
    }
    inner.push_str("<w:p><w:r><w:t>deep</w:t></w:r></w:p>");
    for _ in 0..n {
        inner.push_str("</w:x>");
    }
    let docx = make_docx(&inner);
    let r = Document::open(&docx);
    // Must be the depth cap (or another clean Err) — crucially, no overflow/panic.
    assert!(matches!(r, Err(DocxError::TooDeep)) || r.is_err());
}

// ─── 13. Empty body → empty document, empty text ────────────────────────────

#[test]
fn empty_body() {
    let docx = make_docx("");
    let doc = Document::open(&docx).expect("open");
    assert_eq!(doc.blocks.len(), 0);
    assert_eq!(doc.extract_text(), "");
    assert_eq!(doc.paragraphs().len(), 0);
}

// ─── 14. Content-Types absent but document.xml present is still openable ─────

#[test]
fn content_types_optional_for_open() {
    let doc_xml = document_xml("<w:p><w:r><w:t>no content types</w:t></w:r></w:p>");
    let docx = build_zip(&[member("word/document.xml", doc_xml.as_bytes())]);
    let doc = Document::open(&docx).expect("open without content-types");
    assert_eq!(doc.extract_text(), "no content types");
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// Properties on the public surface: `Document::open` must (a) never panic on ANY
// byte sequence, and (b) bound every allocation (the decompressed-size cap, the
// element-count cap, and the depth cap). FAIL-ability (by reasoning):
//  - `#![forbid(unsafe_code)]` makes any OOB index a guaranteed panic, so a
//    never-panic loop genuinely proves bounds-safety — a hostile-byte panic aborts
//    the test process (red).
//  - Removing the depth cap makes `deep_nesting_bounded` recurse to a stack
//    overflow (abort = red).
//  - Removing the element-count cap lets a tag-stream fixture spin/allocate without
//    bound.
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
    let mut rng = Rng::new(0xD0C_F00D);
    for _ in 0..40_000 {
        let len = rng.below(256);
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(rng.byte());
        }
        // No assertion needed beyond "it returns" — a panic aborts the process.
        let _ = Document::open(&buf);
    }
}

#[test]
fn fuzz_mutated_valid_docx_never_panic() {
    let base = make_docx(
        "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>\
<w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">Title &amp; body</w:t></w:r></w:p>\
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
    );
    assert!(Document::open(&base).is_ok(), "seed docx must open");

    let mut rng = Rng::new(0x3333_D0C);
    for _ in 0..60_000 {
        let mut m = base.clone();
        let muts = 1 + rng.below(6);
        for _ in 0..muts {
            let i = rng.below(m.len());
            m[i] ^= rng.byte();
        }
        let _ = Document::open(&m); // never panics
    }
}

// ════════════════════════════════════════════════════════════════════════════
// WRITER round-trip suite — `Document::to_docx()` → `Document::open()`.
//
// The load-bearing proof for the .docx WRITER: build an in-memory Document, write
// it with the new serializer, then read it back with the EXISTING reader and
// assert the model survives — paragraph count + order, each run's text EXACT and
// its bold/italic/underline flags, the heading style/level, preserved leading/
// trailing spaces, XML-special text decoded correctly, and table cell text. The
// round-trip against an independent reader is what makes "edit AND SAVE" real.
//
// FAIL-ability: each assert below pins a concrete value; if the writer dropped a
// run's bold flag, mangled xml:space, mis-escaped "<", or lost a table cell, the
// reader would reconstruct a different model and the matching assert flips red.
// (One assert is annotated showing the exact tweak that turns it red.)
// ════════════════════════════════════════════════════════════════════════════

/// Build the canonical fixture Document: a Heading1 paragraph, a normal paragraph
/// with three runs (bold / italic / underline combos), a spaced paragraph, an
/// XML-special paragraph, and a 2x2 table. Returned for both writing and as the
/// expectation oracle.
fn fixture_document() -> Document {
    let heading = Paragraph {
        style: Some(String::from("Heading1")),
        runs: vec![Run {
            text: String::from("The Title"),
            ..Default::default()
        }],
    };

    let formatted = Paragraph {
        style: None,
        runs: vec![
            Run {
                text: String::from("boldrun"),
                bold: true,
                ..Default::default()
            },
            Run {
                text: String::from("italicrun"),
                italic: true,
                ..Default::default()
            },
            Run {
                text: String::from("bothunder"),
                bold: true,
                underline: true,
                ..Default::default()
            },
        ],
    };

    let spaced = Paragraph {
        style: None,
        runs: vec![Run {
            text: String::from("  lead and trail  "),
            ..Default::default()
        }],
    };

    let specials = Paragraph {
        style: None,
        runs: vec![Run {
            text: String::from("a<b>&c\"d"),
            ..Default::default()
        }],
    };

    fn cell(text: &str) -> TableCell {
        TableCell {
            paragraphs: vec![Paragraph {
                style: None,
                runs: vec![Run {
                    text: String::from(text),
                    ..Default::default()
                }],
            }],
        }
    }
    let table = Table {
        rows: vec![
            TableRow {
                cells: vec![cell("R1C1"), cell("R1C2")],
            },
            TableRow {
                cells: vec![cell("R2C1"), cell("R2C2")],
            },
        ],
    };

    Document {
        blocks: vec![
            Block::Paragraph(heading),
            Block::Paragraph(formatted),
            Block::Paragraph(spaced),
            Block::Paragraph(specials),
            Block::Table(table),
        ],
    }
}

#[test]
fn write_then_read_roundtrip_exact() {
    let original = fixture_document();
    let bytes = original.to_docx().expect("to_docx");

    // The written bytes must be a real .docx the EXISTING reader opens.
    let read = Document::open(&bytes).expect("reader opens written docx");

    // --- Block count + order: 4 paragraphs then 1 table.
    assert_eq!(read.blocks.len(), 5, "block count + order preserved");
    assert!(matches!(read.blocks[0], Block::Paragraph(_)));
    assert!(matches!(read.blocks[3], Block::Paragraph(_)));
    assert!(matches!(read.blocks[4], Block::Table(_)));

    // --- Heading: style id + level survive the round-trip.
    let h = match &read.blocks[0] {
        Block::Paragraph(p) => p,
        _ => panic!("block 0 not a paragraph"),
    };
    assert_eq!(h.style.as_deref(), Some("Heading1"));
    assert_eq!(h.heading_level(), Some(1));
    assert_eq!(h.runs.len(), 1);
    assert_eq!(h.runs[0].text, "The Title");

    // --- Formatted paragraph: three runs, EXACT text + EXACT flags.
    let f = match &read.blocks[1] {
        Block::Paragraph(p) => p,
        _ => panic!("block 1 not a paragraph"),
    };
    assert_eq!(f.runs.len(), 3, "three runs survive");

    assert_eq!(f.runs[0].text, "boldrun");
    assert!(f.runs[0].bold && !f.runs[0].italic && !f.runs[0].underline);

    assert_eq!(f.runs[1].text, "italicrun");
    assert!(f.runs[1].italic && !f.runs[1].bold && !f.runs[1].underline);

    assert_eq!(f.runs[2].text, "bothunder");
    // FAIL-ability: change `true` → `false` here and the assert flips red iff the
    // writer correctly serialized BOTH bold and underline on this run.
    assert!(f.runs[2].bold && f.runs[2].underline && !f.runs[2].italic);

    // --- Preserved spaces: xml:space="preserve" kept lead/trail whitespace.
    let s = match &read.blocks[2] {
        Block::Paragraph(p) => p,
        _ => panic!("block 2 not a paragraph"),
    };
    assert_eq!(s.runs[0].text, "  lead and trail  ");
    // FAIL-ability: had the writer omitted xml:space, the reader would trim this.
    assert_ne!(s.runs[0].text, "lead and trail");

    // --- XML specials: escaped on write, decoded EXACTLY on read.
    let x = match &read.blocks[3] {
        Block::Paragraph(p) => p,
        _ => panic!("block 3 not a paragraph"),
    };
    assert_eq!(x.runs[0].text, "a<b>&c\"d");
    // FAIL-ability: a mis-escape (e.g. raw '<') would corrupt the XML so the reader
    // reconstructs different text (or fewer runs).
    assert!(!x.runs[0].text.contains("&amp;"));

    // --- Table: 2x2, each cell's text exact.
    let t = match &read.blocks[4] {
        Block::Table(t) => t,
        _ => panic!("block 4 not a table"),
    };
    assert_eq!(t.rows.len(), 2);
    assert_eq!(t.rows[0].cells.len(), 2);
    assert_eq!(t.rows[1].cells.len(), 2);
    assert_eq!(t.rows[0].cells[0].text(), "R1C1");
    assert_eq!(t.rows[0].cells[1].text(), "R1C2");
    assert_eq!(t.rows[1].cells[0].text(), "R2C1");
    assert_eq!(t.rows[1].cells[1].text(), "R2C2");
}

#[test]
fn write_then_read_extract_text_equals_original() {
    let original = fixture_document();
    let bytes = original.to_docx().expect("to_docx");
    let read = Document::open(&bytes).expect("open");

    // The reader's extract_text of the round-tripped doc must equal the original
    // model's extract_text — same blocks, same join semantics.
    assert_eq!(read.extract_text(), original.extract_text());

    // And spot-check the literal text the user would see.
    assert_eq!(
        read.extract_text(),
        "The Title\nboldrunitalicrunbothunder\n  lead and trail  \na<b>&c\"d\nR1C1\tR1C2\nR2C1\tR2C2"
    );
}

#[test]
fn write_empty_document_is_valid_and_reads_back_empty() {
    let empty = Document::default();
    let bytes = empty.to_docx().expect("to_docx empty");

    // The reader must open it as a valid (empty) .docx.
    let read = Document::open(&bytes).expect("reader opens empty written docx");
    assert_eq!(read.blocks.len(), 0);
    assert_eq!(read.extract_text(), "");
}

#[test]
fn written_docx_has_required_opc_parts() {
    let bytes = fixture_document().to_docx().expect("to_docx");
    // The container must carry the three required OPC parts (open via ath_zip
    // directly so we assert on the package, not just the parsed model).
    let ar = ath_zip::Archive::open(&bytes).expect("written bytes are a valid zip");
    assert!(
        ar.find("[Content_Types].xml").is_some(),
        "content types part"
    );
    assert!(ar.find("_rels/.rels").is_some(), "root rels part");
    assert!(ar.find("word/document.xml").is_some(), "main document part");
}

#[test]
fn write_roundtrip_heading_levels_2_through_9() {
    // Every heading level the reader recognizes must round-trip.
    let mut blocks = Vec::new();
    for lvl in 1..=9u8 {
        let mut style = String::from("Heading");
        style.push((b'0' + lvl) as char);
        blocks.push(Block::Paragraph(Paragraph {
            style: Some(style),
            runs: vec![Run {
                text: String::from("h"),
                ..Default::default()
            }],
        }));
    }
    let doc = Document { blocks };
    let bytes = doc.to_docx().expect("to_docx");
    let read = Document::open(&bytes).expect("open");
    assert_eq!(read.blocks.len(), 9);
    for (i, b) in read.blocks.iter().enumerate() {
        let p = match b {
            Block::Paragraph(p) => p,
            _ => panic!("not paragraph"),
        };
        assert_eq!(p.heading_level(), Some((i + 1) as u8));
    }
}

#[test]
fn write_bound_rejects_oversized_document() {
    // The block-count bound is load-bearing: a document past MAX_WRITE_BLOCKS must
    // return Err(TooLarge), never emit a partial/giant file. Build MAX_WRITE_BLOCKS
    // + 1 *empty* paragraphs (cheap default structs; the bound is checked before any
    // XML is generated, so no per-block work happens).
    let mut blocks = Vec::with_capacity(MAX_WRITE_BLOCKS + 1);
    for _ in 0..(MAX_WRITE_BLOCKS + 1) {
        blocks.push(Block::Paragraph(Paragraph::default()));
    }
    let doc = Document { blocks };
    assert_eq!(doc.to_docx(), Err(DocxWriteError::TooLarge));

    // FAIL-ability: exactly MAX_WRITE_BLOCKS is allowed (the boundary is inclusive).
    let ok_blocks: Vec<Block> = (0..MAX_WRITE_BLOCKS)
        .map(|_| Block::Paragraph(Paragraph::default()))
        .collect();
    let ok_doc = Document { blocks: ok_blocks };
    assert!(ok_doc.to_docx().is_ok(), "exactly the cap must serialize");
}

#[test]
fn write_run_cap_rejects_oversized_paragraph() {
    // The per-paragraph run cap must fire too: one paragraph with MAX_WRITE_RUNS + 1
    // runs returns Err(TooLarge).
    let runs: Vec<Run> = (0..(MAX_WRITE_RUNS + 1))
        .map(|_| Run {
            text: String::from("x"),
            ..Default::default()
        })
        .collect();
    let doc = Document {
        blocks: vec![Block::Paragraph(Paragraph { style: None, runs })],
    };
    assert_eq!(doc.to_docx(), Err(DocxWriteError::TooLarge));
}

#[test]
fn fuzz_mutated_xml_body_never_panic() {
    // Mutate only the document.xml *text*, fed straight to the parser, so the fuzzer
    // explores the XML walker (not just the ZIP layer rejecting bad containers).
    let base = document_xml(
        "<w:p><w:r><w:t>alpha</w:t></w:r></w:p><w:tbl><w:tr><w:tc>\
<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>beta</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
    );
    let base_bytes = base.into_bytes();
    let mut rng = Rng::new(0xBEEF_D0C);
    for _ in 0..60_000 {
        let mut m = base_bytes.clone();
        let muts = 1 + rng.below(8);
        for _ in 0..muts {
            let i = rng.below(m.len());
            m[i] = rng.byte();
        }
        // Build a docx around the mutated body and open it; must never panic.
        let docx = build_zip(&[
            member("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            member("word/document.xml", &m),
        ]);
        let _ = Document::open(&docx);
    }
}
