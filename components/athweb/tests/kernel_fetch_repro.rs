//! Repro of the kernel webview bundled-fetch path (host KAT).
use athnet::http1::MockTransport;
use athweb::loader::{self, Mime};

fn canned(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

#[test]
fn bundled_home_fetches_and_parses() {
    let body = "<!DOCTYPE html><html><body>\
        <h1>RaeWeb</h1>\
        <p>The native web surface. No Electron tax.</p>\
        <a href=\"rae://about\">About RaeWeb</a>\
        </body></html>";
    let mut t = MockTransport::new(canned(body));
    let res = loader::fetch_document("http://localhost/", &mut t);
    match res {
        Ok(r) => {
            println!(
                "status={} mime={:?} bytes={}",
                r.status,
                r.mime,
                r.bytes.len()
            );
            assert!(
                r.mime == Mime::Html || r.mime == Mime::Other,
                "mime was {:?}",
                r.mime
            );
            let dom = athweb::parse_html(&r.as_text());
            println!("dom_nodes={}", athweb::count_dom_nodes(&dom));
        }
        Err(e) => panic!("fetch failed: {:?}", e),
    }
}
