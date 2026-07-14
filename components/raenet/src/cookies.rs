//! RFC 6265 cookie jar for the HTTP client (the live `http1` fetch path).
//!
//! Real browsing and any login session need cookies. This is a correct, hostile-
//! input-safe (never-panic) jar: it parses `Set-Cookie`, enforces domain-match
//! (§5.1.3) and path-match (§5.1.4) — including the host-only distinction and the
//! path-boundary rule that a naive `starts_with` gets wrong (`/foo` must NOT match
//! `/foobar`) — and emits the `Cookie:` request header for matching requests.
//! `secure` cookies are withheld over plain http. The dead `https.rs` cookie
//! scaffolding (unwired, with the `starts_with` path bug) is superseded by this.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lowercased, no leading dot.
    pub domain: String,
    /// Always starts with `/`.
    pub path: String,
    /// True when the cookie had no `Domain` attribute → exact-host match only
    /// (must NOT be sent to subdomains).
    pub host_only: bool,
    pub secure: bool,
    /// Unix seconds; `None` = a session cookie (kept until the jar is cleared).
    pub expires: Option<u64>,
}

impl Cookie {
    /// Parse one `Set-Cookie` header value against the request host/path. Returns
    /// `None` for a malformed name/value, an empty name, or a `Domain` attribute
    /// the request host does not domain-match (RFC 6265 §5.3). Never panics.
    pub fn parse(set_cookie: &str, req_host: &str, req_path: &str, now: u64) -> Option<Cookie> {
        let mut parts = set_cookie.split(';');
        let nv = parts.next()?.trim();
        let eq = nv.find('=')?;
        let name = nv[..eq].trim();
        if name.is_empty() {
            return None;
        }
        let value = nv[eq + 1..].trim().to_string();

        let host = req_host.to_ascii_lowercase();
        let mut domain = host.clone();
        let mut host_only = true;
        let mut path: Option<String> = None;
        let mut secure = false;
        let mut max_age: Option<i64> = None;
        let mut expires_attr: Option<u64> = None;

        for attr in parts {
            let attr = attr.trim();
            let (k, v) = match attr.find('=') {
                Some(i) => (attr[..i].trim(), attr[i + 1..].trim()),
                None => (attr, ""),
            };
            if k.eq_ignore_ascii_case("Domain") {
                let d = v.trim_start_matches('.').to_ascii_lowercase();
                if !d.is_empty() {
                    // A Domain the request host can't claim is rejected outright.
                    if host == d || host.ends_with(&format!(".{d}")) {
                        domain = d;
                        host_only = false;
                    } else {
                        return None;
                    }
                }
            } else if k.eq_ignore_ascii_case("Path") {
                if v.starts_with('/') {
                    path = Some(v.to_string());
                }
            } else if k.eq_ignore_ascii_case("Secure") {
                secure = true;
            } else if k.eq_ignore_ascii_case("Max-Age") {
                max_age = v.parse::<i64>().ok();
            } else if k.eq_ignore_ascii_case("Expires") {
                expires_attr = parse_http_date(v);
            }
            // `HttpOnly`/`SameSite` do not affect a programmatic client jar.
        }

        let path = path.unwrap_or_else(|| default_path(req_path));
        // Max-Age takes precedence over Expires (RFC 6265 §5.3).
        let expires = match max_age {
            Some(s) if s <= 0 => Some(0), // delete: already expired
            Some(s) => Some(now.saturating_add(s as u64)),
            None => expires_attr,
        };

        Some(Cookie {
            name: name.to_string(),
            value,
            domain,
            path,
            host_only,
            secure,
            expires,
        })
    }

    fn is_expired(&self, now: u64) -> bool {
        self.expires.map_or(false, |e| now >= e)
    }

    /// RFC 6265 §5.1.3 domain-match (host-only ⇒ exact host).
    fn domain_matches(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        if host == self.domain {
            return true;
        }
        if self.host_only {
            return false;
        }
        host.ends_with(&format!(".{}", self.domain))
    }

    /// RFC 6265 §5.1.4 path-match: equal, or a prefix that ends on a `/` boundary
    /// — so `/foo` matches `/foo`, `/foo/`, `/foo/bar` but NOT `/foobar`.
    fn path_matches(&self, req_path: &str) -> bool {
        let req = req_path.split('?').next().unwrap_or("/");
        if req == self.path {
            return true;
        }
        if req.starts_with(&self.path) {
            if self.path.ends_with('/') {
                return true;
            }
            return req.as_bytes().get(self.path.len()) == Some(&b'/');
        }
        false
    }
}

/// Default-path per RFC 6265 §5.1.4: the request path up to (excluding) the last
/// `/`, or `/` if none / it is the root.
/// Parse an HTTP `Expires` date to epoch seconds. Handles IMF-fixdate
/// ("Sun, 06 Nov 1994 08:49:37 GMT") and the RFC 850 2-digit-year form; returns
/// None on anything it can't read (such cookies stay session cookies). Never panics.
fn parse_http_date(s: &str) -> Option<u64> {
    let cleaned = s.replace([',', '-'], " ");
    let parts: alloc::vec::Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() < 5 {
        return None;
    }
    // parts[0] = weekday (ignored); day month year time follow.
    let day: u32 = parts[1].parse().ok()?;
    let month = month_num(parts[2])?;
    let mut year: i64 = parts[3].parse().ok()?;
    if year < 100 {
        // RFC 6265 §5.1.1 two-digit year rule.
        year += if year < 69 { 2000 } else { 1900 };
    }
    let t: alloc::vec::Vec<&str> = parts[4].split(':').collect();
    if t.len() != 3 {
        return None;
    }
    let hh: i64 = t[0].parse().ok()?;
    let mm: i64 = t[1].parse().ok()?;
    let ss: i64 = t[2].parse().ok()?;
    if day == 0 || day > 31 || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let epoch = days * 86400 + hh * 3600 + mm * 60 + ss;
    if epoch < 0 {
        Some(0)
    } else {
        Some(epoch as u64)
    }
}

fn month_num(m: &str) -> Option<u32> {
    Some(match m.to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// Days since 1970-01-01 (Howard Hinnant's days_from_civil). Valid for any Gregorian
/// date; `..`-style proleptic for years < 1.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mc = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mc + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn default_path(req_path: &str) -> String {
    let p = req_path.split('?').next().unwrap_or("/");
    if !p.starts_with('/') {
        return String::from("/");
    }
    match p.rfind('/') {
        Some(0) | None => String::from("/"),
        Some(i) => p[..i].to_string(),
    }
}

/// A simple in-memory cookie jar. The HTTP client threads it through requests:
/// [`store`](Self::store) the `Set-Cookie`s from a response, then
/// [`cookie_header`](Self::cookie_header) for the next request.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self {
            cookies: Vec::new(),
        }
    }

    /// Store (or replace) a cookie from one `Set-Cookie` value. A `Max-Age<=0`
    /// (or otherwise already-expired) cookie deletes the matching stored cookie.
    pub fn store(&mut self, set_cookie: &str, req_host: &str, req_path: &str, now: u64) {
        let Some(c) = Cookie::parse(set_cookie, req_host, req_path, now) else {
            return;
        };
        // A cookie is keyed by (name, domain, path) — replace any existing one.
        self.cookies
            .retain(|e| !(e.name == c.name && e.domain == c.domain && e.path == c.path));
        if !c.is_expired(now) {
            self.cookies.push(c);
        }
    }

    /// The `Cookie:` header value for a request, or `None` if nothing matches.
    /// Cookies with longer paths sort first (RFC 6265 §5.4).
    pub fn cookie_header(&self, host: &str, path: &str, secure: bool, now: u64) -> Option<String> {
        let mut matched: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                !c.is_expired(now)
                    && c.domain_matches(host)
                    && c.path_matches(path)
                    && (!c.secure || secure)
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        matched.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        let mut out = String::new();
        for (i, c) in matched.iter().enumerate() {
            if i > 0 {
                out.push_str("; ");
            }
            out.push_str(&c.name);
            out.push('=');
            out.push_str(&c.value);
        }
        Some(out)
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_host_only() {
        let c = Cookie::parse("sid=abc123", "example.com", "/app/page", 0).unwrap();
        assert_eq!(c.name, "sid");
        assert_eq!(c.value, "abc123");
        assert_eq!(c.domain, "example.com");
        assert!(c.host_only);
        assert_eq!(c.path, "/app"); // default-path of /app/page
        assert!(!c.secure);
        assert_eq!(c.expires, None);
    }

    #[test]
    fn host_only_not_sent_to_subdomain() {
        let mut jar = CookieJar::new();
        jar.store("sid=x", "example.com", "/", 0);
        // Same host -> sent.
        assert_eq!(
            jar.cookie_header("example.com", "/", false, 0),
            Some("sid=x".into())
        );
        // Subdomain -> NOT sent (host-only).
        assert_eq!(jar.cookie_header("www.example.com", "/", false, 0), None);
    }

    #[test]
    fn domain_cookie_sent_to_subdomain_but_rejects_foreign() {
        let mut jar = CookieJar::new();
        jar.store("sid=x; Domain=example.com", "www.example.com", "/", 0);
        assert_eq!(
            jar.cookie_header("www.example.com", "/", false, 0),
            Some("sid=x".into())
        );
        assert_eq!(
            jar.cookie_header("example.com", "/", false, 0),
            Some("sid=x".into())
        );
        // A Domain the request host can't claim is rejected at parse time.
        assert!(Cookie::parse("sid=x; Domain=evil.com", "www.example.com", "/", 0).is_none());
    }

    #[test]
    fn path_match_boundary() {
        // The bug the https.rs version has: /foo must NOT match /foobar.
        let mut jar = CookieJar::new();
        jar.store("a=1; Path=/foo", "h", "/", 0);
        assert_eq!(jar.cookie_header("h", "/foo", false, 0), Some("a=1".into()));
        assert_eq!(
            jar.cookie_header("h", "/foo/bar", false, 0),
            Some("a=1".into())
        );
        assert_eq!(jar.cookie_header("h", "/foobar", false, 0), None);
        assert_eq!(jar.cookie_header("h", "/", false, 0), None);
    }

    #[test]
    fn secure_cookie_withheld_over_http() {
        let mut jar = CookieJar::new();
        jar.store("s=1; Secure", "h", "/", 0);
        assert_eq!(jar.cookie_header("h", "/", false, 0), None); // http
        assert_eq!(jar.cookie_header("h", "/", true, 0), Some("s=1".into())); // https
    }

    #[test]
    fn max_age_expiry_and_deletion() {
        let mut jar = CookieJar::new();
        jar.store("t=1; Max-Age=100", "h", "/", 1000);
        assert_eq!(jar.cookie_header("h", "/", false, 1050), Some("t=1".into()));
        assert_eq!(jar.cookie_header("h", "/", false, 1100), None); // expired at 1100
                                                                    // Max-Age<=0 deletes.
        jar.store("t=1; Max-Age=600", "h", "/", 0);
        assert!(!jar.is_empty());
        jar.store("t=1; Max-Age=0", "h", "/", 0);
        assert!(jar.is_empty(), "Max-Age=0 must delete the cookie");
    }

    #[test]
    fn replace_same_key_and_join_multiple() {
        let mut jar = CookieJar::new();
        jar.store("a=1", "h", "/", 0);
        jar.store("a=2", "h", "/", 0); // same (name,domain,path) -> replace
        jar.store("b=3", "h", "/", 0);
        assert_eq!(jar.len(), 2);
        let hdr = jar.cookie_header("h", "/", false, 0).unwrap();
        assert!(hdr.contains("a=2") && hdr.contains("b=3") && !hdr.contains("a=1"));
        assert!(hdr.contains("; "));
    }

    #[test]
    fn malformed_never_panics() {
        for s in [
            "",
            "=noval",
            "; ; ;",
            "novalue",
            "a",
            "  =  ",
            "a=b; Domain=",
        ] {
            let mut jar = CookieJar::new();
            jar.store(s, "h", "/p", 0); // must not panic
        }
    }

    #[test]
    fn cookie_expires_date_parsing() {
        let now = 1_000_000u64;
        // A future Expires (no Max-Age) -> a persistent cookie, not expired now.
        let c = Cookie::parse(
            "sid=abc; Expires=Wed, 09 Jun 2100 10:18:14 GMT",
            "example.com",
            "/",
            now,
        )
        .unwrap();
        assert!(c.expires.is_some(), "Expires must set an expiry");
        assert!(c.expires.unwrap() > now, "future expiry > now");
        assert!(!c.is_expired(now));
        // An already-past Expires -> expired.
        let c2 = Cookie::parse(
            "sid=abc; Expires=Thu, 01 Jan 1970 00:00:10 GMT",
            "example.com",
            "/",
            now,
        )
        .unwrap();
        assert!(c2.is_expired(now), "past Expires -> expired");
        // Max-Age takes precedence over Expires.
        let c3 = Cookie::parse(
            "sid=abc; Max-Age=100; Expires=Thu, 01 Jan 1970 00:00:10 GMT",
            "example.com",
            "/",
            now,
        )
        .unwrap();
        assert_eq!(c3.expires, Some(now + 100), "Max-Age wins over Expires");
        // An unparseable Expires -> session cookie (no expiry), never panics.
        let c4 = Cookie::parse("sid=abc; Expires=garbage", "example.com", "/", now).unwrap();
        assert_eq!(c4.expires, None);
    }

    #[test]
    fn http_date_epoch_anchor() {
        // Sanity-check the date math against a known epoch second.
        // Thu, 01 Jan 1970 00:00:00 GMT == 0.
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        // Fri, 13 Feb 2009 23:31:30 GMT == 1234567890.
        assert_eq!(
            parse_http_date("Fri, 13 Feb 2009 23:31:30 GMT"),
            Some(1_234_567_890)
        );
    }
}
