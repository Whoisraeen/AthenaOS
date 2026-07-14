//! POP3 (RFC 1939) minimal client state machine.
//!
//! POP3 is a simple request/response protocol: every command gets a single-line
//! status reply beginning with `+OK` or `-ERR`; multi-line responses (LIST, RETR)
//! are terminated by a line containing only `.` (with dot-unstuffing of body
//! lines that begin with `.`). This module implements the common verbs a reader
//! needs — `USER` / `PASS` / `STAT` / `LIST` / `RETR` / `DELE` / `QUIT` — over the
//! same [`MailTransport`] seam, so it is host-KAT-able with no network.
//!
//! Hostile-byte posture: multi-line responses are bounded by
//! [`crate::limits::MAX_REPLY_LINES`] and total body bytes by
//! [`crate::limits::MAX_LITERAL`]; a server that never sends the terminating `.`
//! yields a typed [`Pop3Error`] on EOF, never an infinite loop.

use crate::message::{FetchedMessage, MessageError};
use crate::{lossy_str, starts_with_ci, strip_crlf, MailTransport, TransportError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// POP3 client errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop3Error {
    Transport(TransportError),
    /// The greeting was not `+OK`.
    BadGreeting(String),
    /// A command returned `-ERR` — carries the server text.
    Err(String),
    /// A reply was neither `+OK` nor `-ERR`.
    Malformed(String),
    /// A multi-line response exceeded its bound.
    ResponseTooLong,
    /// A RETR body exceeded [`crate::limits::MAX_LITERAL`].
    BodyTooLarge,
    /// Wrong protocol state for the command.
    WrongState,
}

impl From<TransportError> for Pop3Error {
    fn from(e: TransportError) -> Self {
        Pop3Error::Transport(e)
    }
}

/// POP3 session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop3State {
    Init,
    /// Greeting read; expecting USER.
    Authorization,
    /// USER accepted; expecting PASS.
    UserSent,
    /// Authenticated; transaction state.
    Transaction,
    /// QUIT sent.
    Closed,
}

/// Mailbox drop statistics from `STAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pop3Stat {
    pub count: u32,
    pub octets: u64,
}

/// The POP3 client.
pub struct Pop3Client {
    state: Pop3State,
}

impl Default for Pop3Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Pop3Client {
    pub fn new() -> Self {
        Pop3Client {
            state: Pop3State::Init,
        }
    }

    pub fn state(&self) -> Pop3State {
        self.state
    }

    /// Read the `+OK` greeting.
    pub fn read_greeting<T: MailTransport>(&mut self, t: &mut T) -> Result<(), Pop3Error> {
        if self.state != Pop3State::Init {
            return Err(Pop3Error::WrongState);
        }
        let (ok, text) = read_status(t)?;
        if ok {
            self.state = Pop3State::Authorization;
            Ok(())
        } else {
            Err(Pop3Error::BadGreeting(text))
        }
    }

    /// `USER name`.
    pub fn user<T: MailTransport>(&mut self, t: &mut T, name: &str) -> Result<(), Pop3Error> {
        if self.state != Pop3State::Authorization {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, &alloc::format!("USER {}", name))?;
        let (ok, text) = read_status(t)?;
        if ok {
            self.state = Pop3State::UserSent;
            Ok(())
        } else {
            Err(Pop3Error::Err(text))
        }
    }

    /// `PASS secret`.
    pub fn pass<T: MailTransport>(&mut self, t: &mut T, secret: &str) -> Result<(), Pop3Error> {
        if self.state != Pop3State::UserSent {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, &alloc::format!("PASS {}", secret))?;
        let (ok, text) = read_status(t)?;
        if ok {
            self.state = Pop3State::Transaction;
            Ok(())
        } else {
            Err(Pop3Error::Err(text))
        }
    }

    /// `STAT` → (message count, total octets).
    pub fn stat<T: MailTransport>(&mut self, t: &mut T) -> Result<Pop3Stat, Pop3Error> {
        if self.state != Pop3State::Transaction {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, "STAT")?;
        let (ok, text) = read_status(t)?;
        if !ok {
            return Err(Pop3Error::Err(text));
        }
        let mut nums = text.split_whitespace();
        let count = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let octets = nums.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok(Pop3Stat { count, octets })
    }

    /// `LIST` → (msg_number, octets) for every message.
    pub fn list<T: MailTransport>(&mut self, t: &mut T) -> Result<Vec<(u32, u64)>, Pop3Error> {
        if self.state != Pop3State::Transaction {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, "LIST")?;
        let (ok, _) = read_status(t)?;
        if !ok {
            return Err(Pop3Error::Err("LIST failed".into()));
        }
        let lines = read_multiline(t)?;
        let mut out = Vec::new();
        for line in &lines {
            let mut parts = line.split_whitespace();
            if let (Some(n), Some(o)) = (parts.next(), parts.next()) {
                if let (Ok(n), Ok(o)) = (n.parse::<u32>(), o.parse::<u64>()) {
                    out.push((n, o));
                }
            }
        }
        Ok(out)
    }

    /// `RETR n` → the raw message bytes (multi-line, dot-unstuffed).
    pub fn retr<T: MailTransport>(&mut self, t: &mut T, n: u32) -> Result<Vec<u8>, Pop3Error> {
        if self.state != Pop3State::Transaction {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, &alloc::format!("RETR {}", n))?;
        let (ok, text) = read_status(t)?;
        if !ok {
            return Err(Pop3Error::Err(text));
        }
        read_multiline_bytes(t)
    }

    /// `RETR n` then parse the message (reusing the message parser / `rae_mime`).
    pub fn retr_message<T: MailTransport>(
        &mut self,
        t: &mut T,
        n: u32,
    ) -> Result<FetchedMessage, Pop3Error> {
        let raw = self.retr(t, n)?;
        FetchedMessage::parse(&raw).map_err(|e: MessageError| {
            // surface a parse failure as a malformed response (never panic)
            Pop3Error::Malformed(alloc::format!("message parse failed: {:?}", e))
        })
    }

    /// `DELE n` — mark a message for deletion.
    pub fn dele<T: MailTransport>(&mut self, t: &mut T, n: u32) -> Result<(), Pop3Error> {
        if self.state != Pop3State::Transaction {
            return Err(Pop3Error::WrongState);
        }
        send_line(t, &alloc::format!("DELE {}", n))?;
        let (ok, text) = read_status(t)?;
        if ok {
            Ok(())
        } else {
            Err(Pop3Error::Err(text))
        }
    }

    /// `QUIT`.
    pub fn quit<T: MailTransport>(&mut self, t: &mut T) -> Result<(), Pop3Error> {
        send_line(t, "QUIT")?;
        let _ = read_status(t);
        self.state = Pop3State::Closed;
        Ok(())
    }
}

fn send_line<T: MailTransport>(t: &mut T, line: &str) -> Result<(), Pop3Error> {
    t.send(line.as_bytes())?;
    t.send(b"\r\n")?;
    Ok(())
}

/// Read a single-line status reply → (is_ok, text_after_status_token).
fn read_status<T: MailTransport>(t: &mut T) -> Result<(bool, String), Pop3Error> {
    let raw = t.recv_line()?;
    let line = strip_crlf(&raw);
    if starts_with_ci(line, b"+OK") {
        let text = lossy_str(&line[3.min(line.len())..]).trim().to_string();
        Ok((true, text))
    } else if starts_with_ci(line, b"-ERR") {
        let text = lossy_str(&line[4.min(line.len())..]).trim().to_string();
        Ok((false, text))
    } else {
        Err(Pop3Error::Malformed(lossy_str(line)))
    }
}

/// Read a multi-line response as lines (dot-unstuffed), terminated by a lone `.`.
/// Bounded by [`crate::limits::MAX_REPLY_LINES`].
fn read_multiline<T: MailTransport>(t: &mut T) -> Result<Vec<String>, Pop3Error> {
    let mut out = Vec::new();
    let mut count = 0;
    loop {
        count += 1;
        if count > crate::limits::MAX_REPLY_LINES {
            return Err(Pop3Error::ResponseTooLong);
        }
        let raw = t.recv_line()?;
        let line = strip_crlf(&raw);
        if line == b"." {
            return Ok(out);
        }
        // dot-unstuff: a body line beginning with ".." has the first dot removed.
        let unstuffed: &[u8] = if line.first() == Some(&b'.') {
            &line[1..]
        } else {
            line
        };
        out.push(lossy_str(unstuffed));
    }
}

/// Read a multi-line response as raw bytes (dot-unstuffed, CRLF-joined),
/// terminated by a lone `.`. Bounded by [`crate::limits::MAX_LITERAL`] total.
fn read_multiline_bytes<T: MailTransport>(t: &mut T) -> Result<Vec<u8>, Pop3Error> {
    let mut out = Vec::new();
    let mut lines = 0;
    loop {
        lines += 1;
        if lines > crate::limits::MAX_REPLY_LINES * 64 {
            // a body can be many lines; allow more than a control multiline but
            // still bounded. Also bounded by total bytes below.
            return Err(Pop3Error::ResponseTooLong);
        }
        let raw = t.recv_line()?;
        let line = strip_crlf(&raw);
        if line == b"." {
            return Ok(out);
        }
        let unstuffed: &[u8] = if line.first() == Some(&b'.') {
            &line[1..]
        } else {
            line
        };
        if out.len() + unstuffed.len() + 2 > crate::limits::MAX_LITERAL {
            return Err(Pop3Error::BodyTooLarge);
        }
        out.extend_from_slice(unstuffed);
        out.extend_from_slice(b"\r\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::ScriptedTransport;

    fn authed_script(rest: &[u8]) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(b"+OK POP3 server ready\r\n");
        s.extend_from_slice(b"+OK user accepted\r\n");
        s.extend_from_slice(b"+OK logged in\r\n");
        s.extend_from_slice(rest);
        s
    }

    fn auth<T: MailTransport>(c: &mut Pop3Client, t: &mut T) {
        c.read_greeting(t).unwrap();
        c.user(t, "alice").unwrap();
        c.pass(t, "secret").unwrap();
        assert_eq!(c.state(), Pop3State::Transaction);
    }

    #[test]
    fn full_auth_emits_expected_commands() {
        let s = authed_script(b"");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        assert!(t.sent_contains("USER alice\r\n"));
        assert!(t.sent_contains("PASS secret\r\n"));
    }

    #[test]
    fn stat_parses_count_and_octets() {
        let s = authed_script(b"+OK 2 320\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        let st = c.stat(&mut t).unwrap();
        assert_eq!(st.count, 2);
        assert_eq!(st.octets, 320);
        assert!(t.sent_contains("STAT\r\n"));
    }

    #[test]
    fn list_parses_messages() {
        let s = authed_script(b"+OK 2 messages\r\n1 120\r\n2 200\r\n.\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        let list = c.list(&mut t).unwrap();
        assert_eq!(list, alloc::vec![(1u32, 120u64), (2u32, 200u64)]);
    }

    #[test]
    fn retr_dot_unstuffs_and_parses() {
        // a message whose body has a line starting with '.', dot-stuffed on wire
        let body = b"+OK 30 octets\r\n\
Subject: Hi\r\n\
\r\n\
normal line\r\n\
..stuffed dot line\r\n\
.\r\n";
        let s = authed_script(body);
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        let raw = c.retr(&mut t, 1).unwrap();
        let text = lossy_str(&raw);
        assert!(text.contains("Subject: Hi"));
        // dot-unstuffing: "..stuffed" → ".stuffed"
        assert!(text.contains(".stuffed dot line"));
        assert!(!text.contains("..stuffed"));
        // parses via the message parser
        let m = c.retr_message_from(&raw).unwrap();
        assert_eq!(m.envelope.subject, "Hi");
    }

    #[test]
    fn dele_and_quit() {
        let s = authed_script(b"+OK marked deleted\r\n+OK bye\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        c.dele(&mut t, 1).unwrap();
        c.quit(&mut t).unwrap();
        assert_eq!(c.state(), Pop3State::Closed);
        assert!(t.sent_contains("DELE 1\r\n"));
        assert!(t.sent_contains("QUIT\r\n"));
    }

    #[test]
    fn err_reply_surfaces() {
        let mut s = Vec::new();
        s.extend_from_slice(b"+OK ready\r\n");
        s.extend_from_slice(b"-ERR no such user\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        c.read_greeting(&mut t).unwrap();
        match c.user(&mut t, "ghost") {
            Err(Pop3Error::Err(m)) => assert!(m.contains("no such user")),
            other => panic!("expected Err, got {:?}", other),
        }
    }

    #[test]
    fn bad_greeting_surfaces() {
        let mut t = ScriptedTransport::new(b"-ERR locked\r\n");
        let mut c = Pop3Client::new();
        assert!(matches!(
            c.read_greeting(&mut t),
            Err(Pop3Error::BadGreeting(_))
        ));
    }

    // ---- hostile / never-panic / never-loop ----

    #[test]
    fn unterminated_multiline_is_graceful_err() {
        // LIST +OK then lines but no terminating "." then EOF
        let s = authed_script(b"+OK list\r\n1 100\r\n2 200\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = Pop3Client::new();
        auth(&mut c, &mut t);
        assert!(matches!(
            c.list(&mut t),
            Err(Pop3Error::Transport(TransportError::Closed))
        ));
    }

    #[test]
    fn malformed_status_is_typed_err() {
        let mut t = ScriptedTransport::new(b"garbage not a status\r\n");
        let mut c = Pop3Client::new();
        assert!(matches!(
            c.read_greeting(&mut t),
            Err(Pop3Error::Malformed(_))
        ));
    }

    #[test]
    fn wrong_state_rejected() {
        let mut t = ScriptedTransport::new(b"+OK ready\r\n");
        let mut c = Pop3Client::new();
        // STAT before auth
        assert_eq!(c.stat(&mut t), Err(Pop3Error::WrongState));
    }
}

// Test-only helper hung off the client to parse an already-fetched RETR body.
#[cfg(test)]
impl Pop3Client {
    fn retr_message_from(&self, raw: &[u8]) -> Result<FetchedMessage, Pop3Error> {
        FetchedMessage::parse(raw).map_err(|e| Pop3Error::Malformed(alloc::format!("{:?}", e)))
    }
}
