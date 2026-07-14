//! # RaeMail — never-panic, `no_std` email client protocol core (SMTP/IMAP/POP3).
//!
//! LEGACY_GAMING_CONCEPT.md §"the common apps people rely on, available and just work"
//! (charter app #5, "mail"): a daily driver has to send and read email. This
//! crate is the protocol core a RaeMail app sits on — it speaks the wire
//! protocols and models the messages, but performs **no I/O itself**.
//!
//! ## Transport-abstracted (the whole reason this is host-KAT-able)
//! Every protocol state machine drives a [`MailTransport`] — a byte pipe with
//! `send` / `recv` / `recv_line`. In production the transport is a TLS-over-TCP
//! socket from `raenet`; in the host KATs it is a [`ScriptedTransport`] that
//! replays canned server bytes and records exactly what the client sent. So the
//! SMTP/IMAP/POP3 dialogs are proven *completely* on the dev box with no live
//! server, no network, no QEMU (`cargo test -p rae_mail`). **The live TLS/TCP
//! wiring over `raenet` is a documented LATER integration step** — this crate has
//! no `raenet` dependency and opens no sockets.
//!
//! ## What it implements
//! - [`smtp`] — RFC 5321 send dialog: greeting, `EHLO` + capability parse
//!   (PIPELINING / STARTTLS / AUTH / SIZE), the STARTTLS *negotiation point* (the
//!   client emits `STARTTLS` and signals the caller to upgrade the transport — the
//!   handshake itself is the transport's job), `AUTH PLAIN` / `AUTH LOGIN`
//!   (base64), `MAIL FROM` / `RCPT TO` / `DATA` with body **dot-stuffing** and the
//!   terminating `.`, `QUIT`. Multi-line replies (`NNN-` continuation vs `NNN `
//!   final) are parsed; 4xx/5xx surface as typed [`SmtpError`].
//! - [`imap`] — RFC 3501 fetch/read dialog: the tagged command / untagged `*`
//!   response model, `CAPABILITY`, `LOGIN`, `SELECT`/`EXAMINE` (EXISTS / RECENT /
//!   FLAGS / UIDVALIDITY), `LIST`, `FETCH` (FLAGS / INTERNALDATE / RFC822.SIZE /
//!   ENVELOPE / `BODY[]`), the critical `{N}` literal octet handling, `SEARCH`,
//!   `STORE`, `LOGOUT`. A fetched `BODY[]` is handed to [`message`] (which reuses
//!   `rae_mime` to classify parts) so a client can list and read mail.
//! - [`pop3`] — RFC 1939 minimal state machine: `USER` / `PASS` / `STAT` /
//!   `LIST` / `RETR` / `DELE` / `QUIT`. (Modeled, not deferred.)
//! - [`message`] — the [`Envelope`], [`Mailbox`], and [`FetchedMessage`] model,
//!   plus the RFC 822 / 2045 message parse (headers, `multipart/*` split,
//!   `Content-Transfer-Encoding` base64 / quoted-printable decode).
//!
//! ## REUSE vs reimplementation (important — `rae_mime` is NOT a message parser)
//! The task brief assumed `rae_mime` parses MIME *messages*. It does not:
//! `rae_mime` is a file-association resolver (extension/magic → MIME type → app).
//! There is therefore no existing RFC 2045 message parser to reuse, so the
//! message-structure parse (headers / multipart / transfer-decoding) lives in
//! [`message`]. We still reuse `rae_mime` for what it *is* good at — classifying a
//! decoded attachment part by its `Content-Type` / filename — and we reuse
//! `rae_encode` for base64 so no base64 codec is reimplemented here.
//!
//! ## Never-panic, never-infinite-loop on hostile bytes
//! A mail server is untrusted input (a malicious or buggy IMAP/POP3/SMTP server
//! can send anything). No `unwrap`/`expect`/`panic`/raw-index-panic is reachable
//! from any public function. Every unbounded quantity is capped before it can
//! exhaust memory or spin forever: line length ([`limits::MAX_LINE`]), a single
//! literal/body ([`limits::MAX_LITERAL`]), the number of untagged responses per
//! command ([`limits::MAX_UNTAGGED`]), reply-continuation lines
//! ([`limits::MAX_REPLY_LINES`]), and the total bytes a transport may yield while
//! satisfying one read ([`limits::MAX_DRAIN`]). A truncated, garbage, or
//! never-terminating server response returns a typed `Err`, never loops or OOMs —
//! proven by the hostile + seeded-fuzz KATs at the bottom of each module.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod imap;
pub mod message;
pub mod pop3;
pub mod smtp;

pub use imap::{ImapClient, ImapError, ImapState};
pub use message::{Address, Envelope, FetchedMessage, Mailbox, MessageError, Part};
pub use pop3::{Pop3Client, Pop3Error, Pop3State};
pub use smtp::{OutgoingMessage, SmtpCaps, SmtpClient, SmtpError, SmtpState};

/// Hard caps applied everywhere a server controls a quantity. Untrusted input
/// can never push any of these past its bound — that is what makes the protocol
/// layer safe to run against a hostile or buggy server.
pub mod limits {
    /// Max bytes in a single protocol line (CRLF-terminated), including the
    /// terminator. RFC 5321 limits SMTP lines to 1000 octets; IMAP/POP3 lines are
    /// short. We allow generous slack but refuse a line that never ends.
    pub const MAX_LINE: usize = 16 * 1024;
    /// Max bytes in a single IMAP `{N}` literal or POP3/IMAP body we will buffer.
    /// A server advertising a larger literal is rejected (no OOM). 32 MiB is far
    /// above any sane single message a client reads inline.
    pub const MAX_LITERAL: usize = 32 * 1024 * 1024;
    /// Max untagged `*` responses accepted while completing one IMAP command. A
    /// server flooding untagged lines forever is bounded here.
    pub const MAX_UNTAGGED: usize = 100_000;
    /// Max continuation lines in one multi-line SMTP / POP3 reply.
    pub const MAX_REPLY_LINES: usize = 1024;
    /// Max total bytes a transport may yield to satisfy a single logical read
    /// (line or literal). A backstop against a transport that returns one byte
    /// forever without ever completing the structure.
    pub const MAX_DRAIN: usize = MAX_LITERAL + MAX_LINE;
}

/// Errors a [`MailTransport`] implementation can raise. Protocol modules wrap
/// these into their own typed error enums so a caller sees one error type per
/// protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The peer closed the connection / no more bytes are available, before the
    /// expected structure (line or N literal octets) was complete.
    Closed,
    /// The transport's own I/O failed; the `String` is a human-readable reason.
    /// (In `no_std` builds the message is still an `alloc::string::String`.)
    Io(String),
    /// A read could not complete within the transport's own deadline.
    Timeout,
}

/// A bidirectional byte pipe the protocol state machines drive.
///
/// This is the single seam between the (pure, host-testable) protocol logic and
/// the (impure, integration-only) network. A production impl wraps a TLS-over-TCP
/// `raenet` socket; the test impl ([`ScriptedTransport`]) replays canned bytes.
///
/// ## Contract
/// - [`send`](MailTransport::send) writes all bytes or returns `Err`.
/// - [`recv_line`](MailTransport::recv_line) reads up to and including the next
///   `\n`, returning the line **without** the trailing CRLF, bounded by
///   [`limits::MAX_LINE`]; a line longer than that is a [`TransportError`] the
///   caller maps to a protocol error rather than buffering unboundedly.
/// - [`recv_exact`](MailTransport::recv_exact) reads exactly `n` raw bytes (used
///   for IMAP literals); `n` is the caller's already-bounded count.
///
/// All three are `&mut self`: a transport is a stateful stream.
pub trait MailTransport {
    /// Send all of `data`, or fail.
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError>;

    /// Read the next CRLF-terminated line, returning it WITHOUT the trailing
    /// `\r\n` (or `\n`). Bounded by [`limits::MAX_LINE`]; the implementation must
    /// not buffer an unbounded line.
    fn recv_line(&mut self) -> Result<Vec<u8>, TransportError>;

    /// Read exactly `n` raw bytes (for length-prefixed IMAP literals). The caller
    /// has already validated `n <= limits::MAX_LITERAL`.
    fn recv_exact(&mut self, n: usize) -> Result<Vec<u8>, TransportError>;
}

/// Strip a trailing `\r\n` or lone `\n` from a line buffer in place-ish (returns a
/// subslice). Never panics on empty input.
pub(crate) fn strip_crlf(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
        if end > 0 && line[end - 1] == b'\r' {
            end -= 1;
        }
    }
    &line[..end]
}

/// Lossy ASCII/UTF-8 view of a byte slice for diagnostics and header values,
/// never panicking on invalid UTF-8 (replaces bad bytes with U+FFFD). Used only
/// for human-facing strings, never for protocol comparisons (those stay byte-wise).
pub(crate) fn lossy_str(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Fast path for ASCII; fall back to char decode for multibyte.
        let b = bytes[i];
        if b < 0x80 {
            s.push(b as char);
            i += 1;
            continue;
        }
        // Try to decode a UTF-8 sequence; on failure emit replacement and advance 1.
        let remaining = &bytes[i..];
        match core::str::from_utf8(remaining) {
            Ok(valid) => {
                s.push_str(valid);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                if good > 0 {
                    // SAFETY-free: from_utf8 on the validated prefix cannot fail.
                    if let Ok(prefix) = core::str::from_utf8(&remaining[..good]) {
                        s.push_str(prefix);
                    }
                    i += good;
                } else {
                    s.push('\u{FFFD}');
                    i += 1;
                }
            }
        }
    }
    s
}

/// Case-insensitive ASCII equality of two byte slices (for command/keyword
/// comparison, e.g. `OK` vs `ok`). Never panics.
pub(crate) fn eq_ascii_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Does `haystack` start with `prefix`, ASCII-case-insensitively? Never panics.
pub(crate) fn starts_with_ci(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack.len() >= prefix.len() && eq_ascii_ci(&haystack[..prefix.len()], prefix)
}

// ===========================================================================
// Host KATs for the shared transport + helpers. Per-protocol KATs live in each
// module. `cargo test -p rae_mail`.
// ===========================================================================
#[cfg(test)]
pub(crate) mod testkit {
    //! A canned, in-test [`MailTransport`]: feeds scripted server bytes to the
    //! client and records every byte the client sent. The whole point of the
    //! transport seam — protocol dialogs proven with zero network.
    use super::*;
    use alloc::collections::VecDeque;
    use alloc::vec::Vec;

    /// Replays `script` (server→client bytes) and captures client→server bytes.
    pub struct ScriptedTransport {
        /// Bytes the server "sends", consumed front-to-back.
        inbox: VecDeque<u8>,
        /// Every byte the client sent, in order (for command-sequence asserts).
        pub sent: Vec<u8>,
        /// If true, `recv_*` returns `Closed` once `inbox` is empty instead of
        /// looping; models a server that hangs up.
        pub close_when_empty: bool,
        /// Backstop so a buggy test can't spin forever reading one transport.
        reads: usize,
    }

    impl ScriptedTransport {
        pub fn new(script: &[u8]) -> Self {
            ScriptedTransport {
                inbox: script.iter().copied().collect(),
                sent: Vec::new(),
                close_when_empty: true,
                reads: 0,
            }
        }

        /// The full client→server transcript as a lossy string (for asserts).
        pub fn sent_str(&self) -> String {
            lossy_str(&self.sent)
        }

        /// Does the recorded client transcript contain `needle` (byte-substring)?
        pub fn sent_contains(&self, needle: &str) -> bool {
            let n = needle.as_bytes();
            if n.is_empty() {
                return true;
            }
            self.sent.windows(n.len()).any(|w| w == n)
        }
    }

    impl MailTransport for ScriptedTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            self.sent.extend_from_slice(data);
            Ok(())
        }

        fn recv_line(&mut self) -> Result<Vec<u8>, TransportError> {
            self.reads += 1;
            if self.reads > 10_000_000 {
                return Err(TransportError::Io("test read runaway".into()));
            }
            let mut line = Vec::new();
            loop {
                match self.inbox.pop_front() {
                    Some(b) => {
                        line.push(b);
                        if b == b'\n' {
                            return Ok(line);
                        }
                        if line.len() > limits::MAX_LINE {
                            return Err(TransportError::Io("line too long".into()));
                        }
                    }
                    None => {
                        if line.is_empty() && self.close_when_empty {
                            return Err(TransportError::Closed);
                        }
                        if self.close_when_empty {
                            // partial line then EOF
                            return Err(TransportError::Closed);
                        }
                        return Err(TransportError::Timeout);
                    }
                }
            }
        }

        fn recv_exact(&mut self, n: usize) -> Result<Vec<u8>, TransportError> {
            self.reads += 1;
            let mut out = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                match self.inbox.pop_front() {
                    Some(b) => out.push(b),
                    None => return Err(TransportError::Closed),
                }
            }
            Ok(out)
        }
    }

    #[test]
    fn scripted_transport_records_sent_and_replays() {
        let mut t = ScriptedTransport::new(b"220 hello\r\n");
        t.send(b"EHLO me\r\n").unwrap();
        let line = t.recv_line().unwrap();
        assert_eq!(strip_crlf(&line), b"220 hello");
        assert!(t.sent_contains("EHLO me"));
        // inbox exhausted → Closed, not a loop.
        assert_eq!(t.recv_line(), Err(TransportError::Closed));
    }

    #[test]
    fn recv_exact_reads_raw_bytes_then_closes() {
        let mut t = ScriptedTransport::new(b"hello world!");
        assert_eq!(t.recv_exact(5).unwrap(), b"hello");
        assert_eq!(t.recv_exact(7).unwrap(), b" world!");
        // nothing left → Closed (not a partial/loop).
        assert_eq!(t.recv_exact(1), Err(TransportError::Closed));
    }

    #[test]
    fn helpers_never_panic_on_empty_and_garbage() {
        assert_eq!(strip_crlf(b""), b"");
        assert_eq!(strip_crlf(b"\n"), b"");
        assert_eq!(strip_crlf(b"\r\n"), b"");
        assert_eq!(strip_crlf(b"x"), b"x");
        assert_eq!(lossy_str(&[0xff, 0xfe, b'a']), "\u{FFFD}\u{FFFD}a");
        assert!(eq_ascii_ci(b"OK", b"ok"));
        assert!(!eq_ascii_ci(b"OK", b"NO"));
        assert!(starts_with_ci(b"a OK done", b"a ok"));
        assert!(!starts_with_ci(b"hi", b"hello"));
    }
}
