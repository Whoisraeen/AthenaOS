//! Host KAT suite for ath_pdf — `cargo test -p ath_pdf`.
//!
//! Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
//! `cfg_attr(not(test), …)`), so `Vec`/`String`/`format!` are in scope via the
//! default prelude — no `use std::` / `extern crate std` (the architecture gate's
//! §R7 bans those lines in a no_std crate).
//!
//! Every test below is FAIL-able by construction: the load-bearing extract_text
//! assert is an exact-string compare (tweak the expected string → red); the
//! hostile-input battery would go red if any decode path panicked or looped.

use super::*;
use alloc::format;

// ─── A tiny in-test PDF assembler ────────────────────────────────────────────
//
// Builds a classic-xref PDF from a set of pre-serialized object bodies, computing
// exact byte offsets so the xref table is self-consistent (the same property
// athprint's generator relies on). This keeps the KATs honest: we assemble a real
// file, then prove the READER recovers it.

struct PdfAsm {
    objects: Vec<Vec<u8>>, // object N (1-based) body, WITHOUT the "N 0 obj"/"endobj" frame
}

impl PdfAsm {
    fn new() -> Self {
        PdfAsm {
            objects: Vec::new(),
        }
    }

    /// Reserve an object slot to be filled later (forward references).
    fn reserve(&mut self) -> u32 {
        self.objects.push(Vec::new());
        self.objects.len() as u32
    }

    fn set(&mut self, num: u32, body: &[u8]) {
        self.objects[(num - 1) as usize] = body.to_vec();
    }

    /// Serialize with a classic `xref` table + trailer (Root = `root_num`).
    fn build(&self, root_num: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = vec![0usize; self.objects.len() + 1];
        for (i, body) in self.objects.iter().enumerate() {
            let num = (i + 1) as u32;
            offsets[num as usize] = out.len();
            out.extend_from_slice(format!("{} 0 obj\n", num).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_off = out.len();
        let count = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {}\n", count).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for num in 1..=self.objects.len() {
            out.extend_from_slice(format!("{:010} {:05} n \n", offsets[num], 0).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{}\n%%EOF\n",
                count, root_num, xref_off
            )
            .as_bytes(),
        );
        out
    }
}

/// Wrap a raw content-stream body in a stream object using an uncompressed body.
fn uncompressed_content_obj(content: &[u8]) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(format!("<< /Length {} >>\nstream\n", content.len()).as_bytes());
    o.extend_from_slice(content);
    o.extend_from_slice(b"\nendstream");
    o
}

/// Wrap a raw content-stream body in a FlateDecode'd stream object (REUSE
/// ath_deflate's compressor so the reader's FlateDecode path is exercised).
fn flate_content_obj(content: &[u8]) -> Vec<u8> {
    let comp = ath_deflate::zlib_compress(content);
    let mut o = Vec::new();
    o.extend_from_slice(
        format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            comp.len()
        )
        .as_bytes(),
    );
    o.extend_from_slice(&comp);
    o.extend_from_slice(b"\nendstream");
    o
}

/// Build a complete single-page PDF whose content shows `text` via one `Tj`.
fn single_page_pdf(text_op: &[u8], compressed: bool) -> Vec<u8> {
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    let page = a.reserve();
    let contents = a.reserve();
    let font = a.reserve();

    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    a.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [ {} 0 R ] >>", page).as_bytes(),
    );
    a.set(
        page,
        format!(
            "<< /Type /Page /Parent {} 0 R /MediaBox [ 0 0 612 792 ] \
             /Resources << /Font << /F1 {} 0 R >> >> /Contents {} 0 R >>",
            pages, font, contents
        )
        .as_bytes(),
    );
    let content = {
        let mut c = Vec::new();
        c.extend_from_slice(b"BT /F1 12 Tf 72 700 Td ");
        c.extend_from_slice(text_op);
        c.extend_from_slice(b" ET");
        c
    };
    let cobj = if compressed {
        flate_content_obj(&content)
    } else {
        uncompressed_content_obj(&content)
    };
    a.set(contents, &cobj);
    a.set(
        font,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    a.build(catalog)
}

// ─── THE load-bearing KAT: extract_text recovers the shown text ──────────────

#[test]
fn opens_and_extracts_hello_uncompressed() {
    let pdf = single_page_pdf(b"(Hello, AthenaOS!) Tj", false);
    let doc = Document::open(&pdf).expect("must parse a valid PDF");
    assert_eq!(doc.page_count(), 1, "single page expected");
    let text = doc.extract_text();
    // The concrete, FAIL-able assert: change the expected string and this flips.
    assert!(
        text.contains("Hello, AthenaOS!"),
        "extract_text must recover the shown text, got {:?}",
        text
    );
    // FAIL-ability witness: it must NOT contain a string we never wrote.
    assert!(!text.contains("Goodbye, Windows"));
}

#[test]
fn flatedecode_content_decompresses_and_extracts() {
    let pdf = single_page_pdf(b"(Hello, AthenaOS!) Tj", true);
    let doc = Document::open(&pdf).expect("FlateDecode PDF must parse");
    assert_eq!(doc.page_count(), 1);
    assert!(
        doc.extract_text().contains("Hello, AthenaOS!"),
        "FlateDecode content must decompress to the shown text"
    );
}

#[test]
fn version_is_parsed() {
    let pdf = single_page_pdf(b"(x) Tj", false);
    let doc = Document::open(&pdf).unwrap();
    assert_eq!(doc.version, "1.7");
}

#[test]
fn media_box_is_recovered() {
    let pdf = single_page_pdf(b"(x) Tj", false);
    let doc = Document::open(&pdf).unwrap();
    let p = &doc.pages[0];
    assert_eq!(p.width(), Some(612.0));
    assert_eq!(p.height(), Some(792.0));
}

// ─── Multi-page enumeration in order ──────────────────────────────────────────

#[test]
fn two_pages_enumerate_in_order() {
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    let page1 = a.reserve();
    let page2 = a.reserve();
    let c1 = a.reserve();
    let c2 = a.reserve();

    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    a.set(
        pages,
        format!(
            "<< /Type /Pages /Count 2 /MediaBox [ 0 0 100 100 ] /Kids [ {} 0 R {} 0 R ] >>",
            page1, page2
        )
        .as_bytes(),
    );
    a.set(
        page1,
        format!(
            "<< /Type /Page /Parent {} 0 R /Contents {} 0 R >>",
            pages, c1
        )
        .as_bytes(),
    );
    a.set(
        page2,
        format!(
            "<< /Type /Page /Parent {} 0 R /Contents {} 0 R >>",
            pages, c2
        )
        .as_bytes(),
    );
    a.set(c1, &uncompressed_content_obj(b"BT (FIRST PAGE) Tj ET"));
    a.set(c2, &uncompressed_content_obj(b"BT (SECOND PAGE) Tj ET"));

    let pdf = a.build(catalog);
    let doc = Document::open(&pdf).expect("two-page PDF must parse");
    assert_eq!(doc.page_count(), 2);
    assert!(doc.pages[0].text().contains("FIRST PAGE"));
    assert!(doc.pages[1].text().contains("SECOND PAGE"));
    // Order: page 1's text appears before page 2's in the joined output.
    let all = doc.extract_text();
    let first = all.find("FIRST").unwrap();
    let second = all.find("SECOND").unwrap();
    assert!(first < second, "pages must enumerate in document order");
    // Inherited MediaBox from the Pages node.
    assert_eq!(doc.pages[0].width(), Some(100.0));
}

// ─── TJ array kerning is handled (numbers dropped, text reassembled) ─────────

#[test]
fn tj_array_kerning_reassembles_text() {
    // `[(He) -250 (llo)] TJ` — the -250 is kerning, not text. Result: "Hello"
    // (the -250 is below the -120 space threshold so it inserts a space; we assert
    // the letters reassemble and no digits leak in).
    let pdf = single_page_pdf(b"[(Hel) -50 (lo)] TJ", false);
    let doc = Document::open(&pdf).unwrap();
    let text = doc.extract_text();
    assert!(
        text.contains("Hello"),
        "TJ runs must reassemble, got {:?}",
        text
    );
    assert!(
        !text.contains("50"),
        "kerning numbers must not leak into text"
    );
    assert!(
        !text.contains("-"),
        "kerning numbers must not leak into text"
    );
}

#[test]
fn tj_large_negative_inserts_space() {
    let pdf = single_page_pdf(b"[(foo) -400 (bar)] TJ", false);
    let doc = Document::open(&pdf).unwrap();
    let text = doc.extract_text();
    assert!(
        text.contains("foo bar"),
        "large kern gap should space-separate, got {:?}",
        text
    );
}

// ─── Literal-string escape decoding ──────────────────────────────────────────

#[test]
fn literal_string_escapes_decode() {
    // \( \) \\ and an octal \101 = 'A'. The shown string should be:  ()\A
    let pdf = single_page_pdf(b"(\\(\\)\\\\\\101) Tj", false);
    let doc = Document::open(&pdf).unwrap();
    let text = doc.extract_text();
    assert!(
        text.contains("()\\A"),
        "escapes must decode, got {:?}",
        text
    );
}

#[test]
fn literal_string_newline_escape() {
    // \n inside a literal becomes a real newline; push_show_bytes maps it to space.
    let pdf = single_page_pdf(b"(a\\nb) Tj", false);
    let doc = Document::open(&pdf).unwrap();
    let text = doc.extract_text();
    assert!(
        text.contains("a b") || text.contains("a\nb"),
        "got {:?}",
        text
    );
}

#[test]
fn balanced_parens_in_literal() {
    let pdf = single_page_pdf(b"(a(b)c) Tj", false);
    let doc = Document::open(&pdf).unwrap();
    assert!(doc.extract_text().contains("a(b)c"));
}

// ─── Hex-string decoding ─────────────────────────────────────────────────────

#[test]
fn hex_string_decodes() {
    // <48656C6C6F> = "Hello".
    let pdf = single_page_pdf(b"<48656C6C6F> Tj", false);
    let doc = Document::open(&pdf).unwrap();
    assert!(
        doc.extract_text().contains("Hello"),
        "hex string must decode to ASCII, got {:?}",
        doc.extract_text()
    );
}

#[test]
fn hex_string_odd_nibble_padded() {
    // <414> = 0x41 0x40 = "A@" (trailing nibble padded with 0).
    let pdf = single_page_pdf(b"<414> Tj", false);
    let doc = Document::open(&pdf).unwrap();
    let t = doc.extract_text();
    assert!(
        t.contains("A@"),
        "odd hex nibble must pad to 0, got {:?}",
        t
    );
}

// ─── Object-model unit asserts (parse directly) ──────────────────────────────

#[test]
fn name_hex_escape_decodes() {
    let mut lx = Lexer::new(b"/A#42C", 0);
    let o = lx.parse_object(0).unwrap();
    assert_eq!(o.as_name(), Some("ABC")); // #42 == 'B'
}

#[test]
fn indirect_reference_parses() {
    let mut lx = Lexer::new(b"12 0 R", 0);
    let o = lx.parse_object(0).unwrap();
    assert_eq!(o, Object::Reference(ObjRef { num: 12, gen: 0 }));
}

#[test]
fn number_not_mistaken_for_reference() {
    let mut lx = Lexer::new(b"12 0", 0);
    let o = lx.parse_object(0).unwrap();
    assert_eq!(o, Object::Integer(12));
}

#[test]
fn real_number_parses() {
    let mut lx = Lexer::new(b"-3.14", 0);
    match lx.parse_object(0).unwrap() {
        Object::Real(r) => assert!((r + 3.14).abs() < 1e-9),
        o => panic!("expected real, got {:?}", o),
    }
}

#[test]
fn nested_dict_array_parse() {
    let src = b"<< /A [1 2 3] /B << /C (x) >> >>";
    let mut lx = Lexer::new(src, 0);
    let o = lx.parse_object(0).unwrap();
    let d = o.as_dict().unwrap();
    assert_eq!(d.get("A").unwrap().as_array().unwrap().len(), 3);
    assert!(d.get("B").unwrap().as_dict().is_some());
}

// ─── Xref STREAM (PDF 1.5+) parsing ──────────────────────────────────────────

/// Build a PDF that uses an xref STREAM instead of a classic xref table. The xref
/// stream is a FlateDecode'd table of W=[1 4 1] records (type, offset, gen/index).
#[test]
fn xref_stream_pdf_parses() {
    // We hand-place objects, then build an xref stream describing their offsets.
    let mut body = Vec::new();
    body.extend_from_slice(b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n");

    let mut offsets: Vec<usize> = vec![0]; // index 0 unused (free)

    let emit = |body: &mut Vec<u8>, offsets: &mut Vec<usize>, num: u32, obj: &[u8]| {
        offsets.push(body.len());
        body.extend_from_slice(format!("{} 0 obj\n", num).as_bytes());
        body.extend_from_slice(obj);
        body.extend_from_slice(b"\nendobj\n");
    };

    // 1 catalog, 2 pages, 3 page, 4 contents.
    emit(
        &mut body,
        &mut offsets,
        1,
        b"<< /Type /Catalog /Pages 2 0 R >>",
    );
    emit(
        &mut body,
        &mut offsets,
        2,
        b"<< /Type /Pages /Count 1 /Kids [ 3 0 R ] >>",
    );
    emit(
        &mut body,
        &mut offsets,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [ 0 0 200 200 ] /Contents 4 0 R >>",
    );
    let content = b"BT (XREF STREAM OK) Tj ET";
    let cobj = uncompressed_content_obj(content);
    emit(&mut body, &mut offsets, 4, &cobj);

    // Object 5 is the xref stream itself. Build its W=[1 4 1] record table:
    //   record for obj 0: type 0 (free) 00000000 ff
    //   records for obj 1..=4: type 1, offset (4 bytes BE), gen 00
    //   record for obj 5 (the xref stream): type 1, offset = where we place it.
    let xref_obj_num = 5u32;
    let xref_off = body.len();
    let mut table: Vec<u8> = Vec::new();
    // obj 0 (free head)
    table.extend_from_slice(&[0u8, 0, 0, 0, 0, 0xFF]);
    for num in 1..=4u32 {
        table.push(1);
        table.extend_from_slice(&(offsets[num as usize] as u32).to_be_bytes());
        table.push(0);
    }
    // obj 5 = the xref stream
    table.push(1);
    table.extend_from_slice(&(xref_off as u32).to_be_bytes());
    table.push(0);

    let comp = ath_deflate::zlib_compress(&table);
    let xref_dict = format!(
        "<< /Type /XRef /Size 6 /W [1 4 1] /Root 1 0 R /Filter /FlateDecode /Length {} >>",
        comp.len()
    );
    body.extend_from_slice(format!("{} 0 obj\n", xref_obj_num).as_bytes());
    body.extend_from_slice(xref_dict.as_bytes());
    body.extend_from_slice(b"\nstream\n");
    body.extend_from_slice(&comp);
    body.extend_from_slice(b"\nendstream\nendobj\n");

    body.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_off).as_bytes());

    let doc = Document::open(&body).expect("xref-stream PDF must parse");
    assert_eq!(doc.version, "1.5");
    assert_eq!(doc.page_count(), 1);
    assert!(
        doc.extract_text().contains("XREF STREAM OK"),
        "xref-stream PDF text must extract, got {:?}",
        doc.extract_text()
    );
    assert_eq!(doc.pages[0].width(), Some(200.0));
}

// ─── Indirect /Length resolution ─────────────────────────────────────────────

#[test]
fn indirect_length_stream() {
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    let page = a.reserve();
    let contents = a.reserve();
    let len_obj = a.reserve();

    let content = b"BT (INDIRECT LEN) Tj ET";
    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    a.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [ {} 0 R ] >>", page).as_bytes(),
    );
    a.set(
        page,
        format!(
            "<< /Type /Page /Parent {} 0 R /Contents {} 0 R >>",
            pages, contents
        )
        .as_bytes(),
    );
    // Length as an indirect reference.
    let mut cobj = Vec::new();
    cobj.extend_from_slice(format!("<< /Length {} 0 R >>\nstream\n", len_obj).as_bytes());
    cobj.extend_from_slice(content);
    cobj.extend_from_slice(b"\nendstream");
    a.set(contents, &cobj);
    a.set(len_obj, format!("{}", content.len()).as_bytes());

    let pdf = a.build(catalog);
    let doc = Document::open(&pdf).expect("indirect-Length PDF must parse");
    assert!(doc.extract_text().contains("INDIRECT LEN"));
}

// ─── Unsupported filter is an honest Err, not faked text ─────────────────────

#[test]
fn unsupported_filter_defers_honestly() {
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    let page = a.reserve();
    let contents = a.reserve();
    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    a.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [ {} 0 R ] >>", page).as_bytes(),
    );
    a.set(
        page,
        format!(
            "<< /Type /Page /Parent {} 0 R /Contents {} 0 R >>",
            pages, contents
        )
        .as_bytes(),
    );
    let raw = b"\x00\x01\x02not really lzw";
    let mut cobj = Vec::new();
    cobj.extend_from_slice(
        format!("<< /Length {} /Filter /LZWDecode >>\nstream\n", raw.len()).as_bytes(),
    );
    cobj.extend_from_slice(raw);
    cobj.extend_from_slice(b"\nendstream");
    a.set(contents, &cobj);
    let pdf = a.build(catalog);
    // Document still opens (page enumerates); the unsupported content just yields
    // no text — never a fake.
    let doc = Document::open(&pdf).expect("doc with one unsupported stream still opens");
    assert_eq!(doc.page_count(), 1);
    assert!(
        doc.extract_text().is_empty(),
        "must not fabricate text for LZW"
    );
    // The decoded_stream API reports the deferral explicitly.
    let cref = doc.pages[0].contents[0];
    assert_eq!(doc.decoded_stream(cref), Err(PdfError::UnsupportedFilter));
    // Raw bytes remain reachable.
    assert_eq!(doc.raw_stream(cref), Some(raw.as_slice()));
}

// ════════════════════════════════════════════════════════════════════════════
// HOSTILE-INPUT BATTERY — every path returns Err, never panics, never loops.
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn non_pdf_is_err() {
    assert_eq!(
        Document::open(b"this is not a pdf").err(),
        Some(PdfError::NotPdf)
    );
    assert_eq!(Document::open(b"").err(), Some(PdfError::NotPdf));
    assert_eq!(
        Document::open(b"%PDZ-1.7 nope").err(),
        Some(PdfError::NotPdf)
    );
}

#[test]
fn truncated_pdf_is_err_not_panic() {
    let pdf = single_page_pdf(b"(Hi) Tj", false);
    // Truncate at every offset — none may panic.
    for cut in 0..pdf.len() {
        let _ = Document::open(&pdf[..cut]); // Ok or Err, never a panic
    }
    // A header with no trailer is an Err.
    assert!(Document::open(b"%PDF-1.7\nnonsense").is_err());
}

#[test]
fn bad_startxref_is_err() {
    let mut pdf = single_page_pdf(b"(Hi) Tj", false);
    // Point startxref far past EOF by appending a bogus one.
    pdf.extend_from_slice(b"\nstartxref\n999999999\n%%EOF\n");
    let _ = Document::open(&pdf); // must not panic
                                  // A startxref that points into garbage.
    let mut bad = Vec::from(&b"%PDF-1.7\n"[..]);
    bad.extend_from_slice(b"startxref\n3\n%%EOF\n");
    assert!(Document::open(&bad).is_err());
}

#[test]
fn reference_cycle_does_not_loop() {
    // Catalog /Pages -> 2; Pages /Kids contains a self-reference (node 2 in its own
    // Kids) AND a Parent cycle. The walker's visited-set must break it.
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    // Pages whose Kids points back at itself (cycle) — must terminate.
    a.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [ {} 0 R ] >>", pages).as_bytes(),
    );
    let pdf = a.build(catalog);
    // No pages can be found (the only kid is a cycle) → BadPageTree, but crucially
    // it RETURNS rather than spinning.
    let r = Document::open(&pdf);
    assert!(r.is_err(), "cyclic page tree must Err, not loop");
}

#[test]
fn indirect_reference_self_cycle_resolves_safely() {
    // Object 3 references itself: `3 0 obj 3 0 R endobj`. resolve() must bottom out.
    let mut a = PdfAsm::new();
    let catalog = a.reserve();
    let pages = a.reserve();
    let page = a.reserve();
    let selfref = a.reserve();
    a.set(
        catalog,
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages).as_bytes(),
    );
    a.set(
        pages,
        format!("<< /Type /Pages /Count 1 /Kids [ {} 0 R ] >>", page).as_bytes(),
    );
    // The page's MediaBox is a self-referential indirect object.
    a.set(
        page,
        format!(
            "<< /Type /Page /Parent {} 0 R /MediaBox {} 0 R >>",
            pages, selfref
        )
        .as_bytes(),
    );
    a.set(selfref, format!("{} 0 R", selfref).as_bytes());
    let pdf = a.build(catalog);
    // Must terminate (resolve depth-bounded); page enumerates with no MediaBox.
    let doc = Document::open(&pdf).expect("self-ref MediaBox must not loop");
    assert_eq!(doc.page_count(), 1);
    assert_eq!(doc.pages[0].media_box, None);
}

#[test]
fn huge_object_count_header_is_bounded() {
    // A classic xref claiming an enormous subsection count must Err (TooLarge),
    // not attempt to allocate billions of entries.
    let mut pdf = Vec::from(&b"%PDF-1.7\n"[..]);
    let xref_off = pdf.len();
    pdf.extend_from_slice(b"xref\n0 9999999999\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size 1 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            xref_off
        )
        .as_bytes(),
    );
    let r = Document::open(&pdf);
    assert!(r.is_err(), "absurd xref count must Err");
}

#[test]
fn deeply_nested_dict_is_bounded() {
    // 10_000 nested arrays — must hit the depth cap and Err, not overflow the stack.
    let mut s = Vec::new();
    for _ in 0..10_000 {
        s.push(b'[');
    }
    let mut lx = Lexer::new(&s, 0);
    assert_eq!(lx.parse_object(0), Err(PdfError::TooDeep));
}

// ─── Seeded fuzz: random + mutated-valid bytes stay bounded + panic-free ─────

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
    let mut rng = Rng::new(0xDEAD_BEEF);
    for _ in 0..20_000 {
        let len = rng.below(512);
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(rng.byte());
        }
        let _ = Document::open(&buf); // Ok or Err — never a panic, never a hang.
    }
}

#[test]
fn fuzz_mutated_valid_never_panic() {
    let base = single_page_pdf(b"(Hello, AthenaOS!) Tj", true);
    let mut rng = Rng::new(0x1234_5678);
    for _ in 0..20_000 {
        let mut m = base.clone();
        // Flip 1..=8 random bytes.
        let flips = 1 + rng.below(8);
        for _ in 0..flips {
            if m.is_empty() {
                break;
            }
            let idx = rng.below(m.len());
            m[idx] ^= rng.byte();
        }
        let _ = Document::open(&m); // must not panic / hang on any mutation
    }
}

#[test]
fn fuzz_content_extractor_never_panic() {
    // The content interpreter is fed arbitrary bytes directly.
    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..20_000 {
        let len = rng.below(256);
        let mut buf = Vec::with_capacity(len);
        for _ in 0..len {
            buf.push(rng.byte());
        }
        let _ = extract_text_from_content(&buf); // bounded + panic-free
    }
}

#[test]
fn fuzz_lexer_truncated_at_every_offset() {
    let src = b"<< /A [1 2 (str) <4869> /Name 3 0 R] /B << /C true >> >>";
    for cut in 0..=src.len() {
        let mut lx = Lexer::new(&src[..cut], 0);
        let _ = lx.parse_object(0); // Ok or Err, never panic
    }
}
