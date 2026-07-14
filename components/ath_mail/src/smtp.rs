//! SMTP (RFC 5321) send-dialog client state machine.
//!
//! Drives a [`MailTransport`] through the send dialog: read the `220` greeting,
//! `EHLO` + parse the capability list, optionally `STARTTLS` (the client emits
//! the command and signals the caller to upgrade the transport — the TLS
//! handshake is the transport's job, a documented integration step), `AUTH`
//! (`PLAIN` / `LOGIN`, base64 via `ath_encode`), `MAIL FROM` / `RCPT TO` / `DATA`
//! with body **dot-stuffing** + the terminating `.`, then `QUIT`. Multi-line
//! replies (`NNN-` continuation vs `NNN ` final) are parsed; any 4xx/5xx surfaces
//! as a typed [`SmtpError`], never a panic.

use crate::{lossy_str, strip_crlf, MailTransport, TransportError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// SMTP client errors. Server reply codes are surfaced, never panicked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpError {
    /// Underlying transport failure.
    Transport(TransportError),
    /// The server's greeting was not `220`.
    BadGreeting(u16),
    /// `EHLO` (and `HELO` fallback) was rejected.
    EhloRejected(u16),
    /// A reply code was malformed (not three ASCII digits).
    MalformedReply(String),
    /// The command was used in the wrong protocol state.
    WrongState,
    /// AUTH failed (server returned a non-2xx/3xx to the auth exchange).
    AuthFailed(u16),
    /// `MAIL FROM` was rejected (4xx/5xx).
    MailFromRejected(u16),
    /// `RCPT TO` was rejected (4xx/5xx) — carries the rejected recipient.
    RcptRejected(u16, String),
    /// `DATA` / message body submission was rejected.
    DataRejected(u16),
    /// The reply continued past [`crate::limits::MAX_REPLY_LINES`].
    ReplyTooLong,
    /// STARTTLS requested but the server did not advertise it.
    StartTlsUnavailable,
}

impl From<TransportError> for SmtpError {
    fn from(e: TransportError) -> Self {
        SmtpError::Transport(e)
    }
}

/// Parsed `EHLO` capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SmtpCaps {
    pub pipelining: bool,
    pub starttls: bool,
    pub auth_plain: bool,
    pub auth_login: bool,
    /// `SIZE` limit if advertised (`SIZE 35882577`).
    pub size: Option<u64>,
    /// Every raw capability keyword line (uppercased first token).
    pub keywords: Vec<String>,
}

/// The SMTP dialog state — a strict, enforced progression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpState {
    /// Before the greeting has been read.
    Init,
    /// Greeting accepted; ready for `EHLO`.
    Greeted,
    /// `EHLO` done; capabilities known; ready for STARTTLS/AUTH/MAIL.
    Ready,
    /// Inside a transaction after `MAIL FROM` accepted.
    MailStarted,
    /// At least one `RCPT TO` accepted.
    RcptDone,
    /// Connection finished (`QUIT` sent).
    Closed,
}

/// A parsed multi-line SMTP reply.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reply {
    code: u16,
    /// Each text line of the reply (without the code prefix), in order.
    lines: Vec<String>,
}

impl Reply {
    fn is_positive(&self) -> bool {
        (200..400).contains(&self.code)
    }
}

/// The SMTP send-dialog client. Borrows a transport per call (it does not own
/// the socket — the caller may need to swap the transport for STARTTLS).
pub struct SmtpClient {
    state: SmtpState,
    caps: SmtpCaps,
}

impl Default for SmtpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SmtpClient {
    pub fn new() -> Self {
        SmtpClient {
            state: SmtpState::Init,
            caps: SmtpCaps::default(),
        }
    }

    pub fn state(&self) -> SmtpState {
        self.state
    }

    pub fn caps(&self) -> &SmtpCaps {
        &self.caps
    }

    /// Read and validate the server greeting (must be `220`).
    pub fn read_greeting<T: MailTransport>(&mut self, t: &mut T) -> Result<(), SmtpError> {
        if self.state != SmtpState::Init {
            return Err(SmtpError::WrongState);
        }
        let reply = read_reply(t)?;
        if reply.code != 220 {
            return Err(SmtpError::BadGreeting(reply.code));
        }
        self.state = SmtpState::Greeted;
        Ok(())
    }

    /// Send `EHLO <domain>` and parse the capability list. Falls back to `HELO`
    /// if the server rejects `EHLO`.
    pub fn ehlo<T: MailTransport>(&mut self, t: &mut T, domain: &str) -> Result<(), SmtpError> {
        if self.state != SmtpState::Greeted {
            return Err(SmtpError::WrongState);
        }
        send_line(t, &alloc::format!("EHLO {}", domain))?;
        let reply = read_reply(t)?;
        if reply.is_positive() {
            self.caps = parse_caps(&reply);
            self.state = SmtpState::Ready;
            return Ok(());
        }
        // HELO fallback for ancient servers.
        send_line(t, &alloc::format!("HELO {}", domain))?;
        let reply2 = read_reply(t)?;
        if reply2.is_positive() {
            self.caps = SmtpCaps::default();
            self.state = SmtpState::Ready;
            Ok(())
        } else {
            Err(SmtpError::EhloRejected(reply2.code))
        }
    }

    /// Emit `STARTTLS` and confirm the server's `220` go-ahead. Returns `Ok(())`
    /// at the point where the **caller must upgrade the transport to TLS** and
    /// then re-`ehlo`. This crate performs no handshake — that is the transport /
    /// `athnet` integration step. Requires the server to have advertised STARTTLS.
    pub fn start_tls<T: MailTransport>(&mut self, t: &mut T) -> Result<(), SmtpError> {
        if self.state != SmtpState::Ready {
            return Err(SmtpError::WrongState);
        }
        if !self.caps.starttls {
            return Err(SmtpError::StartTlsUnavailable);
        }
        send_line(t, "STARTTLS")?;
        let reply = read_reply(t)?;
        if reply.code != 220 {
            return Err(SmtpError::StartTlsUnavailable);
        }
        // Caller upgrades the transport now, then calls `ehlo` again. After a TLS
        // upgrade the protocol restarts at the greeted point conceptually; we move
        // back to Greeted so a re-EHLO is the required next step.
        self.state = SmtpState::Greeted;
        Ok(())
    }

    /// `AUTH PLAIN` with the standard `\0user\0pass` base64 blob (RFC 4616).
    pub fn auth_plain<T: MailTransport>(
        &mut self,
        t: &mut T,
        user: &str,
        pass: &str,
    ) -> Result<(), SmtpError> {
        if self.state != SmtpState::Ready {
            return Err(SmtpError::WrongState);
        }
        let mut blob = Vec::new();
        blob.push(0u8);
        blob.extend_from_slice(user.as_bytes());
        blob.push(0u8);
        blob.extend_from_slice(pass.as_bytes());
        let b64 = ath_encode::base64_encode(&blob);
        send_line(t, &alloc::format!("AUTH PLAIN {}", b64))?;
        let reply = read_reply(t)?;
        if reply.code == 235 {
            Ok(())
        } else {
            Err(SmtpError::AuthFailed(reply.code))
        }
    }

    /// `AUTH LOGIN`: server prompts (`334`) for base64 username then password.
    pub fn auth_login<T: MailTransport>(
        &mut self,
        t: &mut T,
        user: &str,
        pass: &str,
    ) -> Result<(), SmtpError> {
        if self.state != SmtpState::Ready {
            return Err(SmtpError::WrongState);
        }
        send_line(t, "AUTH LOGIN")?;
        let r1 = read_reply(t)?;
        if r1.code != 334 {
            return Err(SmtpError::AuthFailed(r1.code));
        }
        send_line(t, &ath_encode::base64_encode(user.as_bytes()))?;
        let r2 = read_reply(t)?;
        if r2.code != 334 {
            return Err(SmtpError::AuthFailed(r2.code));
        }
        send_line(t, &ath_encode::base64_encode(pass.as_bytes()))?;
        let r3 = read_reply(t)?;
        if r3.code == 235 {
            Ok(())
        } else {
            Err(SmtpError::AuthFailed(r3.code))
        }
    }

    /// `MAIL FROM:<addr>`. Must be in `Ready` state.
    pub fn mail_from<T: MailTransport>(&mut self, t: &mut T, addr: &str) -> Result<(), SmtpError> {
        if self.state != SmtpState::Ready {
            return Err(SmtpError::WrongState);
        }
        send_line(t, &alloc::format!("MAIL FROM:<{}>", addr))?;
        let reply = read_reply(t)?;
        if reply.is_positive() {
            self.state = SmtpState::MailStarted;
            Ok(())
        } else {
            Err(SmtpError::MailFromRejected(reply.code))
        }
    }

    /// `RCPT TO:<addr>`. A 4xx/5xx surfaces as [`SmtpError::RcptRejected`].
    pub fn rcpt_to<T: MailTransport>(&mut self, t: &mut T, addr: &str) -> Result<(), SmtpError> {
        if self.state != SmtpState::MailStarted && self.state != SmtpState::RcptDone {
            return Err(SmtpError::WrongState);
        }
        send_line(t, &alloc::format!("RCPT TO:<{}>", addr))?;
        let reply = read_reply(t)?;
        if reply.is_positive() {
            self.state = SmtpState::RcptDone;
            Ok(())
        } else {
            Err(SmtpError::RcptRejected(reply.code, addr.to_string()))
        }
    }

    /// `DATA` + the dot-stuffed message body + the terminating `.`. The `body`
    /// is the full RFC 822 message (headers + blank line + body); each line is
    /// CRLF-normalized and any line beginning with `.` is dot-stuffed to `..`
    /// (RFC 5321 §4.5.2) so a body line can never be mistaken for the terminator.
    pub fn data<T: MailTransport>(&mut self, t: &mut T, body: &str) -> Result<(), SmtpError> {
        if self.state != SmtpState::RcptDone {
            return Err(SmtpError::WrongState);
        }
        send_line(t, "DATA")?;
        let r1 = read_reply(t)?;
        if r1.code != 354 {
            return Err(SmtpError::DataRejected(r1.code));
        }
        // Send the dot-stuffed body. Normalize line endings to CRLF.
        let stuffed = dot_stuff(body);
        t.send(stuffed.as_bytes())?;
        // Terminating sequence: CRLF . CRLF
        t.send(b"\r\n.\r\n")?;
        let r2 = read_reply(t)?;
        if r2.is_positive() {
            self.state = SmtpState::Ready;
            Ok(())
        } else {
            Err(SmtpError::DataRejected(r2.code))
        }
    }

    /// Convenience: send a fully-built [`OutgoingMessage`] through `DATA` after
    /// `MAIL FROM` / one `RCPT TO` per recipient. Assumes `Ready` state (post-auth).
    pub fn send_message<T: MailTransport>(
        &mut self,
        t: &mut T,
        msg: &OutgoingMessage,
    ) -> Result<(), SmtpError> {
        self.mail_from(t, &msg.from_addr)?;
        for r in msg.recipients() {
            self.rcpt_to(t, &r)?;
        }
        let serialized = msg.serialize();
        self.data(t, &serialized)
    }

    /// `QUIT`.
    pub fn quit<T: MailTransport>(&mut self, t: &mut T) -> Result<(), SmtpError> {
        send_line(t, "QUIT")?;
        let _ = read_reply(t); // a server may close before replying; ignore.
        self.state = SmtpState::Closed;
        Ok(())
    }
}

/// Dot-stuff a body: normalize bare `\n` to `\r\n` and prefix any line starting
/// with `.` with an extra `.`. Returns the body WITHOUT a trailing terminator
/// (the caller appends `\r\n.\r\n`). Never panics.
fn dot_stuff(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + 16);
    let mut at_line_start = true;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if at_line_start && b == b'.' {
            out.push('.'); // stuff
        }
        if b == b'\r' {
            // collapse CRLF to one CRLF; handle lone CR as CRLF
            out.push_str("\r\n");
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                i += 1;
            }
            at_line_start = true;
        } else if b == b'\n' {
            out.push_str("\r\n");
            at_line_start = true;
        } else {
            out.push(b as char);
            at_line_start = false;
        }
        i += 1;
    }
    out
}

/// An outgoing message builder: headers + body, optionally one MIME attachment.
#[derive(Debug, Clone, Default)]
pub struct OutgoingMessage {
    /// Envelope-level sender (the `MAIL FROM` addr-spec).
    pub from_addr: String,
    pub from_display: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    /// RFC 5322 `Date:` value (caller supplies; we do not read a clock).
    pub date: String,
    /// Globally-unique `Message-ID` value WITHOUT the angle brackets.
    pub message_id: String,
    /// The plain-text body.
    pub body: String,
    /// Optional single attachment: (filename, content_type, raw bytes).
    pub attachment: Option<(String, String, Vec<u8>)>,
}

impl OutgoingMessage {
    pub fn new(from: &str, to: &str, subject: &str, body: &str) -> Self {
        OutgoingMessage {
            from_addr: from.to_string(),
            from_display: String::new(),
            to: alloc::vec![to.to_string()],
            cc: Vec::new(),
            subject: subject.to_string(),
            date: String::new(),
            message_id: String::new(),
            body: body.to_string(),
            attachment: None,
        }
    }

    /// All recipients (To + Cc) for `RCPT TO`.
    pub fn recipients(&self) -> Vec<String> {
        let mut v = self.to.clone();
        v.extend(self.cc.iter().cloned());
        v
    }

    /// Serialize the full RFC 822 message. If an attachment is present, builds a
    /// `multipart/mixed` body with the text part and a base64 attachment part
    /// (reusing `ath_encode`). Otherwise a simple `text/plain` message.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        let from_hdr = if self.from_display.is_empty() {
            self.from_addr.clone()
        } else {
            alloc::format!("{} <{}>", self.from_display, self.from_addr)
        };
        out.push_str(&alloc::format!("From: {}\r\n", from_hdr));
        out.push_str(&alloc::format!("To: {}\r\n", self.to.join(", ")));
        if !self.cc.is_empty() {
            out.push_str(&alloc::format!("Cc: {}\r\n", self.cc.join(", ")));
        }
        out.push_str(&alloc::format!("Subject: {}\r\n", self.subject));
        if !self.date.is_empty() {
            out.push_str(&alloc::format!("Date: {}\r\n", self.date));
        }
        if !self.message_id.is_empty() {
            out.push_str(&alloc::format!("Message-ID: <{}>\r\n", self.message_id));
        }
        out.push_str("MIME-Version: 1.0\r\n");

        match &self.attachment {
            None => {
                out.push_str("Content-Type: text/plain; charset=utf-8\r\n");
                out.push_str("\r\n");
                out.push_str(&self.body);
            }
            Some((fname, ctype, bytes)) => {
                // a fixed, simple boundary (no clock/RNG dependency here).
                let boundary = "raemail-boundary-7a3f";
                out.push_str(&alloc::format!(
                    "Content-Type: multipart/mixed; boundary=\"{}\"\r\n\r\n",
                    boundary
                ));
                // text part
                out.push_str(&alloc::format!("--{}\r\n", boundary));
                out.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
                out.push_str(&self.body);
                out.push_str("\r\n");
                // attachment part
                out.push_str(&alloc::format!("--{}\r\n", boundary));
                out.push_str(&alloc::format!(
                    "Content-Type: {}; name=\"{}\"\r\n",
                    ctype,
                    fname
                ));
                out.push_str("Content-Transfer-Encoding: base64\r\n");
                out.push_str(&alloc::format!(
                    "Content-Disposition: attachment; filename=\"{}\"\r\n\r\n",
                    fname
                ));
                // base64, wrapped at 76 chars per RFC 2045.
                let b64 = ath_encode::base64_encode(bytes);
                for chunk in b64.as_bytes().chunks(76) {
                    out.push_str(&lossy_str(chunk));
                    out.push_str("\r\n");
                }
                out.push_str(&alloc::format!("--{}--\r\n", boundary));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Wire helpers
// ---------------------------------------------------------------------------

fn send_line<T: MailTransport>(t: &mut T, line: &str) -> Result<(), SmtpError> {
    t.send(line.as_bytes())?;
    t.send(b"\r\n")?;
    Ok(())
}

/// Read a (possibly multi-line) SMTP reply: lines of the form `NNN-text`
/// (continuation) ending with `NNN text` (final). Bounded by
/// [`crate::limits::MAX_REPLY_LINES`]. Never panics or loops on garbage.
fn read_reply<T: MailTransport>(t: &mut T) -> Result<Reply, SmtpError> {
    let mut lines = Vec::new();
    let mut code: Option<u16> = None;
    let mut count = 0;
    loop {
        count += 1;
        if count > crate::limits::MAX_REPLY_LINES {
            return Err(SmtpError::ReplyTooLong);
        }
        let raw = t.recv_line()?;
        let line = strip_crlf(&raw);
        // need at least "NNN" (3 digits)
        if line.len() < 3 || !line[..3].iter().all(|b| b.is_ascii_digit()) {
            return Err(SmtpError::MalformedReply(lossy_str(line)));
        }
        let this_code =
            (line[0] - b'0') as u16 * 100 + (line[1] - b'0') as u16 * 10 + (line[2] - b'0') as u16;
        // first code wins; a mismatched continuation code is tolerated but the
        // first one is authoritative.
        if code.is_none() {
            code = Some(this_code);
        }
        // separator: '-' = more lines, ' ' (or end) = final.
        let sep = line.get(3).copied();
        let text = if line.len() > 4 {
            lossy_str(&line[4..])
        } else {
            String::new()
        };
        lines.push(text);
        match sep {
            Some(b'-') => continue, // continuation
            _ => break,             // ' ', None, or anything else → final
        }
    }
    Ok(Reply {
        code: code.unwrap_or(0),
        lines,
    })
}

fn parse_caps(reply: &Reply) -> SmtpCaps {
    let mut caps = SmtpCaps::default();
    // The first line is the greeting text; capability keywords follow.
    for (idx, line) in reply.lines.iter().enumerate() {
        if idx == 0 {
            continue; // domain greeting line
        }
        let upper = line.to_ascii_uppercase();
        let mut parts = upper.split_whitespace();
        let kw = parts.next().unwrap_or("");
        match kw {
            "PIPELINING" => caps.pipelining = true,
            "STARTTLS" => caps.starttls = true,
            "AUTH" => {
                for mech in parts.clone() {
                    match mech {
                        "PLAIN" => caps.auth_plain = true,
                        "LOGIN" => caps.auth_login = true,
                        _ => {}
                    }
                }
            }
            "SIZE" => {
                if let Some(n) = parts.next() {
                    caps.size = n.parse::<u64>().ok();
                }
            }
            _ => {}
        }
        if !kw.is_empty() {
            caps.keywords.push(kw.to_string());
        }
    }
    caps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::ScriptedTransport;

    /// A canned server that walks a full successful send.
    fn full_send_script() -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(b"220 mail.example.com ESMTP\r\n");
        // EHLO multi-line reply
        s.extend_from_slice(b"250-mail.example.com greets you\r\n");
        s.extend_from_slice(b"250-PIPELINING\r\n");
        s.extend_from_slice(b"250-SIZE 35882577\r\n");
        s.extend_from_slice(b"250-STARTTLS\r\n");
        s.extend_from_slice(b"250-AUTH PLAIN LOGIN\r\n");
        s.extend_from_slice(b"250 HELP\r\n");
        // AUTH PLAIN
        s.extend_from_slice(b"235 2.7.0 Authentication successful\r\n");
        // MAIL FROM
        s.extend_from_slice(b"250 2.1.0 Ok\r\n");
        // RCPT TO
        s.extend_from_slice(b"250 2.1.5 Ok\r\n");
        // DATA
        s.extend_from_slice(b"354 End data with <CR><LF>.<CR><LF>\r\n");
        // after body
        s.extend_from_slice(b"250 2.0.0 Ok: queued as 12345\r\n");
        // QUIT
        s.extend_from_slice(b"221 2.0.0 Bye\r\n");
        s
    }

    #[test]
    fn full_successful_send_emits_exact_command_sequence() {
        let mut t = ScriptedTransport::new(&full_send_script());
        let mut c = SmtpClient::new();

        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "client.local").unwrap();
        // capabilities parsed
        assert!(c.caps().pipelining);
        assert!(c.caps().starttls);
        assert!(c.caps().auth_plain);
        assert!(c.caps().auth_login);
        assert_eq!(c.caps().size, Some(35882577));

        c.auth_plain(&mut t, "user", "pass").unwrap();
        c.mail_from(&mut t, "ada@client.local").unwrap();
        c.rcpt_to(&mut t, "bob@example.com").unwrap();
        c.data(&mut t, "Subject: Hi\r\n\r\nhello body\r\n").unwrap();
        c.quit(&mut t).unwrap();
        assert_eq!(c.state(), SmtpState::Closed);

        // EXACT command-sequence assertions (the load-bearing proof).
        let sent = t.sent_str();
        assert!(t.sent_contains("EHLO client.local\r\n"));
        // AUTH PLAIN base64 of \0user\0pass
        let expected_b64 = ath_encode::base64_encode(b"\0user\0pass");
        assert!(t.sent_contains(&alloc::format!("AUTH PLAIN {}\r\n", expected_b64)));
        assert!(t.sent_contains("MAIL FROM:<ada@client.local>\r\n"));
        assert!(t.sent_contains("RCPT TO:<bob@example.com>\r\n"));
        assert!(t.sent_contains("DATA\r\n"));
        assert!(t.sent_contains("\r\n.\r\n")); // terminating dot
        assert!(t.sent_contains("QUIT\r\n"));
        // ordering: EHLO before MAIL FROM before DATA before QUIT
        let p_ehlo = sent.find("EHLO").unwrap();
        let p_mail = sent.find("MAIL FROM").unwrap();
        let p_data = sent.find("DATA").unwrap();
        let p_quit = sent.find("QUIT").unwrap();
        assert!(p_ehlo < p_mail && p_mail < p_data && p_data < p_quit);
    }

    #[test]
    fn auth_plain_base64_is_exactly_right() {
        // RFC 4616: authcid is \0user\0pass; verify the exact base64.
        let blob = b"\0alice\0s3cret";
        let expected = ath_encode::base64_encode(blob);
        // decode it back to be sure the test oracle is itself right
        let round = ath_encode::base64_decode(&expected).unwrap();
        assert_eq!(round, blob);

        let mut s = Vec::new();
        s.extend_from_slice(b"220 ok\r\n250 EHLO ok\r\n235 ok\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "h").unwrap();
        c.auth_plain(&mut t, "alice", "s3cret").unwrap();
        assert!(t.sent_contains(&alloc::format!("AUTH PLAIN {}\r\n", expected)));
    }

    #[test]
    fn rcpt_550_surfaces_as_typed_error() {
        let mut s = Vec::new();
        s.extend_from_slice(b"220 ok\r\n250 EHLO ok\r\n250 mail ok\r\n");
        s.extend_from_slice(b"550 5.1.1 No such user\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "h").unwrap();
        c.mail_from(&mut t, "a@h").unwrap();
        let err = c.rcpt_to(&mut t, "ghost@example.com").unwrap_err();
        assert_eq!(
            err,
            SmtpError::RcptRejected(550, "ghost@example.com".to_string())
        );
    }

    #[test]
    fn bad_greeting_surfaces() {
        let mut t = ScriptedTransport::new(b"554 No service\r\n");
        let mut c = SmtpClient::new();
        assert_eq!(c.read_greeting(&mut t), Err(SmtpError::BadGreeting(554)));
    }

    #[test]
    fn multiline_ehlo_caps_parse() {
        let mut s = Vec::new();
        s.extend_from_slice(b"220 ok\r\n");
        s.extend_from_slice(b"250-greet\r\n250-PIPELINING\r\n250-AUTH LOGIN\r\n250 SIZE 1000\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "h").unwrap();
        assert!(c.caps().pipelining);
        assert!(c.caps().auth_login);
        assert!(!c.caps().auth_plain);
        assert_eq!(c.caps().size, Some(1000));
    }

    #[test]
    fn body_line_starting_with_dot_is_stuffed() {
        let stuffed = dot_stuff("normal line\r\n.hidden command\r\n..already\r\n");
        // ".hidden" → "..hidden"; "..already" → "...already"
        assert!(stuffed.contains("\r\n..hidden command\r\n"));
        assert!(stuffed.contains("\r\n...already\r\n"));
        // a leading dot on the very first line is also stuffed
        let first = dot_stuff(".start");
        assert!(first.starts_with(".."));
    }

    #[test]
    fn dot_stuffing_in_data_dialog() {
        let mut s = Vec::new();
        s.extend_from_slice(
            b"220 ok\r\n250 EHLO\r\n250 mail\r\n250 rcpt\r\n354 go\r\n250 queued\r\n",
        );
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "h").unwrap();
        c.mail_from(&mut t, "a@h").unwrap();
        c.rcpt_to(&mut t, "b@h").unwrap();
        // body has a line that is just "." which MUST be stuffed to ".."
        c.data(&mut t, "Subject: x\r\n\r\nline1\r\n.\r\nline2\r\n")
            .unwrap();
        // the lone-dot body line must appear as "\r\n..\r\n" in the wire bytes,
        // and the real terminator is "\r\n.\r\n" at the very end.
        assert!(t.sent_contains("\r\n..\r\n"));
        assert!(t.sent.ends_with(b"\r\n.\r\n"));
    }

    #[test]
    fn outgoing_message_serialize_simple() {
        let m = OutgoingMessage::new("ada@x.com", "bob@y.com", "Hello", "Hi Bob!");
        let s = m.serialize();
        assert!(s.contains("From: ada@x.com\r\n"));
        assert!(s.contains("To: bob@y.com\r\n"));
        assert!(s.contains("Subject: Hello\r\n"));
        assert!(s.contains("Content-Type: text/plain; charset=utf-8\r\n"));
        assert!(s.ends_with("Hi Bob!"));
        // round-trips through the message parser
        let parsed = crate::message::FetchedMessage::parse(s.as_bytes()).unwrap();
        assert_eq!(parsed.envelope.subject, "Hello");
        assert_eq!(parsed.body_text(), "Hi Bob!");
    }

    #[test]
    fn outgoing_message_with_attachment_is_multipart_and_round_trips() {
        let mut m = OutgoingMessage::new("ada@x.com", "bob@y.com", "Files", "see attached");
        m.attachment = Some((
            "hi.txt".to_string(),
            "text/plain".to_string(),
            b"attached body".to_vec(),
        ));
        let s = m.serialize();
        assert!(s.contains("multipart/mixed"));
        assert!(s.contains("Content-Transfer-Encoding: base64"));
        let parsed = crate::message::FetchedMessage::parse(s.as_bytes()).unwrap();
        assert_eq!(parsed.parts.len(), 2);
        assert!(parsed.parts[0].text().contains("see attached"));
        assert_eq!(parsed.parts[1].filename, "hi.txt");
        assert_eq!(parsed.parts[1].body, b"attached body");
    }

    #[test]
    fn send_message_convenience_walks_full_transaction() {
        let mut s = Vec::new();
        s.extend_from_slice(
            b"220 ok\r\n250 EHLO\r\n250 mail\r\n250 rcpt\r\n250 rcpt2\r\n354 go\r\n250 queued\r\n",
        );
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        c.ehlo(&mut t, "h").unwrap();
        let mut m = OutgoingMessage::new("ada@x.com", "bob@y.com", "Hi", "body");
        m.cc = alloc::vec!["carol@z.com".to_string()];
        c.send_message(&mut t, &m).unwrap();
        assert!(t.sent_contains("RCPT TO:<bob@y.com>\r\n"));
        assert!(t.sent_contains("RCPT TO:<carol@z.com>\r\n"));
    }

    // ---- hostile / never-panic / never-loop ----

    #[test]
    fn malformed_reply_code_is_typed_error_not_panic() {
        let mut t = ScriptedTransport::new(b"abc not a code\r\n");
        let mut c = SmtpClient::new();
        match c.read_greeting(&mut t) {
            Err(SmtpError::MalformedReply(_)) => {}
            other => panic!("expected MalformedReply, got {:?}", other),
        }
    }

    #[test]
    fn server_that_never_ends_continuation_is_bounded() {
        // a server that sends "250-" forever (no final " ") must hit the line cap,
        // not loop. Build more than MAX_REPLY_LINES continuation lines.
        let mut s = Vec::new();
        s.extend_from_slice(b"220 ok\r\n");
        for _ in 0..(crate::limits::MAX_REPLY_LINES + 5) {
            s.extend_from_slice(b"250-more\r\n");
        }
        let mut t = ScriptedTransport::new(&s);
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        assert_eq!(c.ehlo(&mut t, "h"), Err(SmtpError::ReplyTooLong));
    }

    #[test]
    fn closed_connection_mid_dialog_is_typed_error() {
        // greeting then immediate EOF before EHLO reply
        let mut t = ScriptedTransport::new(b"220 ok\r\n");
        let mut c = SmtpClient::new();
        c.read_greeting(&mut t).unwrap();
        assert_eq!(
            c.ehlo(&mut t, "h"),
            Err(SmtpError::Transport(TransportError::Closed))
        );
    }

    #[test]
    fn wrong_state_is_rejected() {
        let mut t = ScriptedTransport::new(b"220 ok\r\n");
        let mut c = SmtpClient::new();
        // calling mail_from before greeting+ehlo
        assert_eq!(c.mail_from(&mut t, "a@h"), Err(SmtpError::WrongState));
    }

    /// Seeded fuzz: feed random bytes as the greeting/EHLO replies; assert no
    /// panic and a typed Err or Ok, never a hang.
    #[test]
    fn fuzz_reply_parser_bounded_and_panic_free() {
        struct Rng(u64);
        impl Rng {
            fn b(&mut self) -> u8 {
                let mut x = self.0;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.0 = x;
                (x & 0xff) as u8
            }
        }
        let mut rng = Rng(0x1234_5678_9abc_def0);
        for _ in 0..3000 {
            let len = (rng.b() as usize) % 200;
            let mut buf = Vec::with_capacity(len + 2);
            for _ in 0..len {
                buf.push(rng.b());
            }
            // ensure the stream terminates so recv_line can't block forever
            buf.extend_from_slice(b"\r\n");
            let mut t = ScriptedTransport::new(&buf);
            let mut c = SmtpClient::new();
            // must return (Ok or Err), never panic / hang
            let _ = c.read_greeting(&mut t);
        }
    }
}
