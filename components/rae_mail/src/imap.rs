//! IMAP (RFC 3501) fetch/read-dialog client state machine.
//!
//! Implements the tagged command / untagged-`*`-response model: every command is
//! prefixed with a generated tag (`A0001`, `A0002`, …) and completes when the
//! server returns a line beginning with that tag and `OK` / `NO` / `BAD`.
//! Untagged `*` lines carry the data (EXISTS, FLAGS, LIST, FETCH, SEARCH, …).
//!
//! ## The literal `{N}` octet handling (the critical, easy-to-get-wrong part)
//! IMAP transmits opaque/large data as a length-prefixed literal: a line ending
//! in `{N}` (or `{N+}`) means "the next N octets are raw data, NOT line-oriented".
//! `BODY[]` bodies arrive this way. We detect a trailing `{N}`, bound `N` by
//! [`crate::limits::MAX_LITERAL`] (a hostile server advertising `{999999999}` is
//! rejected, never OOMs), then `recv_exact(N)` the raw bytes — they are read by
//! count, never by newline, so a body containing CRLFs is read intact.
//!
//! ## Hostile-byte posture
//! Untagged responses per command are bounded ([`crate::limits::MAX_UNTAGGED`]),
//! literal size is bounded, and a server that never sends the tagged completion
//! line yields a typed [`ImapError`] when the transport closes — never an
//! infinite loop, never a panic.

use crate::message::{parse_imap_envelope, Envelope, FetchedMessage, Mailbox};
use crate::{eq_ascii_ci, lossy_str, starts_with_ci, strip_crlf, MailTransport, TransportError};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// IMAP client errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapError {
    Transport(TransportError),
    /// The server's greeting was not `* OK` / `* PREAUTH`.
    BadGreeting(String),
    /// A tagged `NO` completion (command failed) — carries the text.
    No(String),
    /// A tagged `BAD` completion (protocol error) — carries the text.
    Bad(String),
    /// A `{N}` literal exceeded [`crate::limits::MAX_LITERAL`].
    LiteralTooLarge(usize),
    /// More than [`crate::limits::MAX_UNTAGGED`] untagged responses for one cmd.
    TooManyResponses,
    /// A response could not be parsed (malformed literal header, etc.).
    Malformed(String),
    /// The command was issued in the wrong protocol state.
    WrongState,
}

impl From<TransportError> for ImapError {
    fn from(e: TransportError) -> Self {
        ImapError::Transport(e)
    }
}

/// Connection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImapState {
    /// Before greeting.
    Init,
    /// Greeting read; not authenticated.
    NotAuthenticated,
    /// After successful LOGIN/AUTHENTICATE.
    Authenticated,
    /// After SELECT/EXAMINE — a mailbox is open.
    Selected,
    /// After LOGOUT.
    LoggedOut,
}

/// A FETCH result for one message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchResult {
    /// The message sequence number (the `* N FETCH` prefix).
    pub seq: u32,
    pub flags: Vec<String>,
    pub internal_date: String,
    pub rfc822_size: Option<u64>,
    /// Parsed ENVELOPE, if requested.
    pub envelope: Option<Envelope>,
    /// Raw `BODY[]` (or `BODY[HEADER]` / `BODY[TEXT]`) octets, if requested.
    pub body: Option<Vec<u8>>,
}

impl FetchResult {
    /// Parse the fetched `BODY[]` into a [`FetchedMessage`] (reusing the message
    /// parser, which in turn reuses `rae_mime` for part classification). Returns
    /// `None` if no body was fetched.
    pub fn parse_body(&self) -> Option<FetchedMessage> {
        self.body
            .as_ref()
            .and_then(|b| FetchedMessage::parse(b).ok())
    }
}

/// One untagged response line, possibly with attached literal data.
struct UntaggedLine {
    /// The full text line (without CRLF). Any literal placeholders `{N}` in the
    /// line have their data appended in `literals` in order of appearance.
    text: String,
    /// Literal payloads encountered while reading this logical response, in order.
    literals: Vec<Vec<u8>>,
}

/// The IMAP client.
pub struct ImapClient {
    state: ImapState,
    tag_counter: u32,
    capabilities: Vec<String>,
    mailbox: Mailbox,
}

impl Default for ImapClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ImapClient {
    pub fn new() -> Self {
        ImapClient {
            state: ImapState::Init,
            tag_counter: 0,
            capabilities: Vec::new(),
            mailbox: Mailbox::default(),
        }
    }

    pub fn state(&self) -> ImapState {
        self.state
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn mailbox(&self) -> &Mailbox {
        &self.mailbox
    }

    fn next_tag(&mut self) -> String {
        self.tag_counter = self.tag_counter.wrapping_add(1);
        alloc::format!("A{:04}", self.tag_counter)
    }

    /// Read the server greeting (`* OK ...` or `* PREAUTH ...`).
    pub fn read_greeting<T: MailTransport>(&mut self, t: &mut T) -> Result<(), ImapError> {
        if self.state != ImapState::Init {
            return Err(ImapError::WrongState);
        }
        let raw = t.recv_line()?;
        let line = strip_crlf(&raw);
        if starts_with_ci(line, b"* OK") {
            self.state = ImapState::NotAuthenticated;
            Ok(())
        } else if starts_with_ci(line, b"* PREAUTH") {
            self.state = ImapState::Authenticated;
            Ok(())
        } else {
            Err(ImapError::BadGreeting(lossy_str(line)))
        }
    }

    /// `CAPABILITY`. Stores the advertised capability keywords.
    pub fn capability<T: MailTransport>(&mut self, t: &mut T) -> Result<Vec<String>, ImapError> {
        let (untagged, _) = self.run_command(t, "CAPABILITY", &[])?;
        let mut caps = Vec::new();
        for u in &untagged {
            if starts_with_ci(u.text.as_bytes(), b"CAPABILITY") {
                for tok in u.text.split_whitespace().skip(1) {
                    caps.push(tok.to_string());
                }
            }
        }
        self.capabilities = caps.clone();
        Ok(caps)
    }

    /// `LOGIN user pass`.
    pub fn login<T: MailTransport>(
        &mut self,
        t: &mut T,
        user: &str,
        pass: &str,
    ) -> Result<(), ImapError> {
        if self.state != ImapState::NotAuthenticated {
            return Err(ImapError::WrongState);
        }
        let cmd = alloc::format!("LOGIN {} {}", quote(user), quote(pass));
        self.run_command(t, &cmd, &[])?;
        self.state = ImapState::Authenticated;
        Ok(())
    }

    /// `AUTHENTICATE PLAIN` with a base64 `\0user\0pass` initial response. (The
    /// server-challenge `+` flow is folded: we send the SASL initial response
    /// inline, which most servers accept; a strict-challenge server would need the
    /// continuation handled by the transport integration.)
    pub fn authenticate_plain<T: MailTransport>(
        &mut self,
        t: &mut T,
        user: &str,
        pass: &str,
    ) -> Result<(), ImapError> {
        if self.state != ImapState::NotAuthenticated {
            return Err(ImapError::WrongState);
        }
        let mut blob = Vec::new();
        blob.push(0u8);
        blob.extend_from_slice(user.as_bytes());
        blob.push(0u8);
        blob.extend_from_slice(pass.as_bytes());
        let b64 = rae_encode::base64_encode(&blob);
        let cmd = alloc::format!("AUTHENTICATE PLAIN {}", b64);
        self.run_command(t, &cmd, &[])?;
        self.state = ImapState::Authenticated;
        Ok(())
    }

    /// `SELECT mailbox` (read-write) — parses EXISTS / RECENT / FLAGS / UIDVALIDITY.
    pub fn select<T: MailTransport>(
        &mut self,
        t: &mut T,
        name: &str,
    ) -> Result<Mailbox, ImapError> {
        self.select_or_examine(t, name, false)
    }

    /// `EXAMINE mailbox` (read-only).
    pub fn examine<T: MailTransport>(
        &mut self,
        t: &mut T,
        name: &str,
    ) -> Result<Mailbox, ImapError> {
        self.select_or_examine(t, name, true)
    }

    fn select_or_examine<T: MailTransport>(
        &mut self,
        t: &mut T,
        name: &str,
        examine: bool,
    ) -> Result<Mailbox, ImapError> {
        if self.state != ImapState::Authenticated && self.state != ImapState::Selected {
            return Err(ImapError::WrongState);
        }
        let verb = if examine { "EXAMINE" } else { "SELECT" };
        let cmd = alloc::format!("{} {}", verb, quote(name));
        let (untagged, _) = self.run_command(t, &cmd, &[])?;

        let mut mbox = Mailbox {
            name: name.to_string(),
            delimiter: '/',
            ..Mailbox::default()
        };
        for u in &untagged {
            let txt = u.text.as_str();
            // "* 3 EXISTS", "* 1 RECENT"
            if let Some(rest) = txt
                .strip_suffix(" EXISTS")
                .or_else(|| suffix_word(txt, "EXISTS"))
            {
                mbox.exists = rest.trim().parse::<u32>().ok();
            } else if let Some(rest) = suffix_word(txt, "RECENT") {
                mbox.recent = rest.trim().parse::<u32>().ok();
            } else if starts_with_ci(txt.as_bytes(), b"FLAGS") {
                mbox.flags = parse_paren_flags(txt);
            } else if let Some(uv) = parse_status_item(txt, "UIDVALIDITY") {
                mbox.uid_validity = uv.parse::<u32>().ok();
            }
        }
        self.mailbox = mbox.clone();
        self.state = ImapState::Selected;
        Ok(mbox)
    }

    /// `LIST "" pattern` — returns the matched mailboxes.
    pub fn list<T: MailTransport>(
        &mut self,
        t: &mut T,
        reference: &str,
        pattern: &str,
    ) -> Result<Vec<Mailbox>, ImapError> {
        if self.state != ImapState::Authenticated && self.state != ImapState::Selected {
            return Err(ImapError::WrongState);
        }
        let cmd = alloc::format!("LIST {} {}", quote(reference), quote(pattern));
        let (untagged, _) = self.run_command(t, &cmd, &[])?;
        let mut out = Vec::new();
        for u in &untagged {
            if starts_with_ci(u.text.as_bytes(), b"LIST") {
                if let Some(mb) = parse_list_line(&u.text) {
                    out.push(mb);
                }
            }
        }
        Ok(out)
    }

    /// `FETCH seq items` for a single sequence number. `items` is the raw fetch
    /// item spec, e.g. `(ENVELOPE BODY[])` or `(FLAGS RFC822.SIZE BODY[HEADER])`.
    pub fn fetch<T: MailTransport>(
        &mut self,
        t: &mut T,
        seq: u32,
        items: &str,
    ) -> Result<Vec<FetchResult>, ImapError> {
        if self.state != ImapState::Selected {
            return Err(ImapError::WrongState);
        }
        let cmd = alloc::format!("FETCH {} {}", seq, items);
        let (untagged, _) = self.run_command(t, &cmd, &[])?;
        let mut results = Vec::new();
        for u in &untagged {
            if let Some(fr) = parse_fetch(u) {
                results.push(fr);
            }
        }
        Ok(results)
    }

    /// `SEARCH criteria` — returns the matched message sequence numbers.
    pub fn search<T: MailTransport>(
        &mut self,
        t: &mut T,
        criteria: &str,
    ) -> Result<Vec<u32>, ImapError> {
        if self.state != ImapState::Selected {
            return Err(ImapError::WrongState);
        }
        let cmd = alloc::format!("SEARCH {}", criteria);
        let (untagged, _) = self.run_command(t, &cmd, &[])?;
        let mut ids = Vec::new();
        for u in &untagged {
            if starts_with_ci(u.text.as_bytes(), b"SEARCH") {
                for tok in u.text.split_whitespace().skip(1) {
                    if let Ok(n) = tok.parse::<u32>() {
                        ids.push(n);
                    }
                }
            }
        }
        Ok(ids)
    }

    /// `STORE seq item value`, e.g. `store(t, 3, "+FLAGS", "(\\Seen)")`.
    /// Returns the updated FETCH FLAGS responses the server emits.
    pub fn store<T: MailTransport>(
        &mut self,
        t: &mut T,
        seq: u32,
        item: &str,
        value: &str,
    ) -> Result<Vec<FetchResult>, ImapError> {
        if self.state != ImapState::Selected {
            return Err(ImapError::WrongState);
        }
        let cmd = alloc::format!("STORE {} {} {}", seq, item, value);
        let (untagged, _) = self.run_command(t, &cmd, &[])?;
        let mut out = Vec::new();
        for u in &untagged {
            if let Some(fr) = parse_fetch(u) {
                out.push(fr);
            }
        }
        Ok(out)
    }

    /// `LOGOUT`.
    pub fn logout<T: MailTransport>(&mut self, t: &mut T) -> Result<(), ImapError> {
        let _ = self.run_command(t, "LOGOUT", &[]);
        self.state = ImapState::LoggedOut;
        Ok(())
    }

    /// Send a tagged command and read the full response (untagged lines until the
    /// tagged completion). Returns the untagged lines + the completion text on
    /// `OK`, or a typed error on `NO`/`BAD`/limit/transport failure.
    fn run_command<T: MailTransport>(
        &mut self,
        t: &mut T,
        command: &str,
        _args: &[&str],
    ) -> Result<(Vec<UntaggedLine>, String), ImapError> {
        let tag = self.next_tag();
        t.send(tag.as_bytes())?;
        t.send(b" ")?;
        t.send(command.as_bytes())?;
        t.send(b"\r\n")?;

        let mut untagged = Vec::new();
        let mut count = 0;
        loop {
            count += 1;
            if count > crate::limits::MAX_UNTAGGED {
                return Err(ImapError::TooManyResponses);
            }
            let line = read_response_line(t)?;
            let bytes = line.text.as_bytes();

            // Tagged completion?
            if starts_with_ci(bytes, tag.as_bytes()) {
                // tag<space>STATUS rest
                let rest = line.text[tag.len()..].trim_start();
                let mut words = rest.splitn(2, ' ');
                let status = words.next().unwrap_or("");
                let text = words.next().unwrap_or("").to_string();
                if eq_ascii_ci(status.as_bytes(), b"OK") {
                    return Ok((untagged, text));
                } else if eq_ascii_ci(status.as_bytes(), b"NO") {
                    return Err(ImapError::No(text));
                } else if eq_ascii_ci(status.as_bytes(), b"BAD") {
                    return Err(ImapError::Bad(text));
                } else {
                    return Err(ImapError::Malformed(rest.to_string()));
                }
            }

            // Untagged response (starts with "* ") or a continuation "+ ".
            if line.text.starts_with("* ") {
                // strip the "* " prefix for parsers
                let mut ul = line;
                ul.text = ul.text[2..].to_string();
                untagged.push(ul);
            } else if line.text.starts_with('+') {
                // a command continuation request with no pending data — ignore.
                continue;
            }
            // any other stray line is tolerated and skipped (defensive)
        }
    }
}

/// Read one logical IMAP response line, resolving any trailing/embedded `{N}`
/// literals by reading exactly N raw octets and continuing the line afterward.
///
/// An IMAP line can contain multiple literals (e.g. a FETCH with several string
/// fields); each `{N}` is replaced inline by a placeholder and its bytes are
/// pushed to `literals`. Bounded by [`crate::limits::MAX_LITERAL`] per literal.
fn read_response_line<T: MailTransport>(t: &mut T) -> Result<UntaggedLine, ImapError> {
    let mut text = String::new();
    let mut literals: Vec<Vec<u8>> = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 1024 {
            // a single logical line with >1024 literal segments is hostile.
            return Err(ImapError::Malformed("too many literal segments".into()));
        }
        let raw = t.recv_line()?;
        let line = strip_crlf(&raw);
        // Does this physical line end with a literal announcement {N} or {N+}?
        if let Some(n) = trailing_literal_len(line)? {
            if n > crate::limits::MAX_LITERAL {
                return Err(ImapError::LiteralTooLarge(n));
            }
            // append the line text up to (but excluding) the "{N}" marker; we keep
            // the marker text so a parser can see where data was, plus the data is
            // recorded in `literals`.
            text.push_str(&lossy_str(line));
            let data = t.recv_exact(n)?;
            if data.len() != n {
                return Err(ImapError::Transport(TransportError::Closed));
            }
            literals.push(data);
            // the octets are followed by more of the same logical line; loop to
            // read the continuation (which itself may carry another literal).
            continue;
        } else {
            text.push_str(&lossy_str(line));
            return Ok(UntaggedLine { text, literals });
        }
    }
}

/// If `line` ends with an IMAP literal announcement `{N}` or `{N+}` (a
/// non-synchronizing literal), return `Some(N)`. Validates N is all digits and in
/// range; a malformed `{...}` returns an error rather than silently mis-reading.
fn trailing_literal_len(line: &[u8]) -> Result<Option<usize>, ImapError> {
    if line.is_empty() || line[line.len() - 1] != b'}' {
        return Ok(None);
    }
    // find the matching '{'
    let open = match line.iter().rposition(|&b| b == b'{') {
        Some(o) => o,
        None => return Ok(None),
    };
    let inner = &line[open + 1..line.len() - 1];
    if inner.is_empty() {
        return Err(ImapError::Malformed("empty literal length".into()));
    }
    // allow a trailing '+' for non-sync literals
    let digits = if inner.last() == Some(&b'+') {
        &inner[..inner.len() - 1]
    } else {
        inner
    };
    if digits.is_empty() || !digits.iter().all(|b| b.is_ascii_digit()) {
        // a '{' '}' that is not a literal length (e.g. part of body text that
        // happened to be on its own) → treat as not-a-literal, not an error.
        return Ok(None);
    }
    // parse with overflow guard
    let mut n: usize = 0;
    for &d in digits {
        n = match n
            .checked_mul(10)
            .and_then(|x| x.checked_add((d - b'0') as usize))
        {
            Some(v) => v,
            None => return Err(ImapError::LiteralTooLarge(usize::MAX)),
        };
    }
    Ok(Some(n))
}

// ---------------------------------------------------------------------------
// Response parsers
// ---------------------------------------------------------------------------

/// Quote an IMAP astring if it needs quoting (contains space/special), else pass.
fn quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
    {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    }
}

/// Extract the number before a trailing keyword, e.g. `"3 EXISTS"` → `Some("3")`.
fn suffix_word<'a>(txt: &'a str, word: &str) -> Option<&'a str> {
    let t = txt.trim_end();
    if t.len() >= word.len() && t[t.len() - word.len()..].eq_ignore_ascii_case(word) {
        Some(t[..t.len() - word.len()].trim_end())
    } else {
        None
    }
}

/// Parse `FLAGS (\Seen \Answered)` → ["\\Seen", "\\Answered"].
fn parse_paren_flags(txt: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(o), Some(c)) = (txt.find('('), txt.rfind(')')) {
        if c > o {
            for f in txt[o + 1..c].split_whitespace() {
                out.push(f.to_string());
            }
        }
    }
    out
}

/// Parse a `STATUS`-style `KEY value` embedded in a bracketed response such as
/// `OK [UIDVALIDITY 3857529045] ...`.
fn parse_status_item(txt: &str, key: &str) -> Option<String> {
    let upper = txt.to_ascii_uppercase();
    let pos = upper.find(&key.to_ascii_uppercase())?;
    let after = &txt[pos + key.len()..];
    let val: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

/// Parse a `LIST (\HasNoChildren) "/" "INBOX"` line.
fn parse_list_line(txt: &str) -> Option<Mailbox> {
    // after the "LIST" keyword: (attrs) "delim" name
    let rest = txt.get(4..)?.trim_start();
    let attributes = parse_paren_flags(rest);
    // find the part after the closing ')'
    let after_attrs = rest.find(')').map(|i| &rest[i + 1..]).unwrap_or(rest);
    let after_attrs = after_attrs.trim_start();
    // delimiter: quoted char or NIL
    let mut tokens = split_imap_tokens(after_attrs);
    let delim = tokens
        .first()
        .and_then(|t| t.chars().next())
        .filter(|c| *c != 'N') // crude NIL guard
        .unwrap_or('/');
    let name = if tokens.len() >= 2 {
        tokens.remove(1)
    } else {
        String::new()
    };
    Some(Mailbox {
        name,
        delimiter: delim,
        attributes,
        ..Mailbox::default()
    })
}

/// Tokenize an IMAP fragment into quoted/unquoted tokens. Never panics.
fn split_imap_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'"' => {
                i += 1;
                let mut tok = String::new();
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        tok.push(bytes[i + 1] as char);
                        i += 2;
                        continue;
                    }
                    tok.push(bytes[i] as char);
                    i += 1;
                }
                i += 1; // closing quote
                out.push(tok);
            }
            _ => {
                let start = i;
                while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
                    i += 1;
                }
                out.push(lossy_str(&bytes[start..i]));
            }
        }
        if out.len() > 1024 {
            break; // bounded
        }
    }
    out
}

/// Parse a `* N FETCH (...)` untagged response into a [`FetchResult`].
/// `BODY[...]` data arrives as the line's recorded literal(s).
fn parse_fetch(u: &UntaggedLine) -> Option<FetchResult> {
    let txt = u.text.trim_start();
    // "N FETCH (...)"
    let mut parts = txt.splitn(2, ' ');
    let seq: u32 = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    if !starts_with_ci(rest.as_bytes(), b"FETCH") {
        return None;
    }
    let mut fr = FetchResult {
        seq,
        ..FetchResult::default()
    };

    // FLAGS (...)
    if let Some(pos) = find_ci(rest, "FLAGS (") {
        let after = &rest[pos + 6..];
        if let Some(close) = after.find(')') {
            for f in after[1..close].split_whitespace() {
                fr.flags.push(f.to_string());
            }
        }
    }
    // RFC822.SIZE N
    if let Some(sz) = parse_status_item(rest, "RFC822.SIZE") {
        fr.rfc822_size = sz.parse::<u64>().ok();
    }
    // INTERNALDATE "..."
    if let Some(pos) = find_ci(rest, "INTERNALDATE ") {
        let after = &rest[pos + 13..];
        let toks = split_imap_tokens(after);
        if let Some(first) = toks.first() {
            fr.internal_date = first.clone();
        }
    }
    // ENVELOPE (...)
    if let Some(pos) = find_ci(rest, "ENVELOPE ") {
        let after = &rest[pos + 8..];
        let env_str = balanced_paren(after);
        if !env_str.is_empty() {
            fr.envelope = Some(parse_imap_envelope(&env_str));
        }
    }
    // BODY[...] — the data is in the recorded literals (first one for a single
    // BODY[] fetch). If there is a literal, that's the body.
    if find_ci(rest, "BODY[").is_some() || find_ci(rest, "RFC822").is_some() {
        if let Some(first) = u.literals.first() {
            fr.body = Some(first.clone());
        }
    }
    Some(fr)
}

/// Case-insensitive substring search; returns the byte index. Never panics.
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    let mut i = 0;
    while i + n.len() <= h.len() {
        if h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Given a string whose first non-space char is '(', return the substring from
/// that '(' through the matching ')'. Bounded; never panics on unbalanced input
/// (returns an empty string). Leading whitespace is tolerated.
fn balanced_paren(s: &str) -> String {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'(') {
        return String::new();
    }
    let mut depth = 0i32;
    let mut in_q = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_q = !in_q,
            b'(' if !in_q => depth += 1,
            b')' if !in_q => {
                depth -= 1;
                if depth == 0 {
                    return lossy_str(&bytes[..=i]);
                }
            }
            _ => {}
        }
        if i > 1_000_000 {
            break; // hard bound
        }
    }
    String::new() // unbalanced
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::ScriptedTransport;

    fn logged_in() -> (ImapClient, Vec<u8>) {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK [CAPABILITY IMAP4rev1] ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN completed\r\n");
        (ImapClient::new(), s)
    }

    #[test]
    fn greeting_and_login_advance_state() {
        let (mut c, mut script) = logged_in();
        let mut t = ScriptedTransport::new(&script);
        let _ = &mut script;
        c.read_greeting(&mut t).unwrap();
        assert_eq!(c.state(), ImapState::NotAuthenticated);
        c.login(&mut t, "user", "pass").unwrap();
        assert_eq!(c.state(), ImapState::Authenticated);
        // exact command emitted (tag + LOGIN)
        assert!(t.sent_contains("A0001 LOGIN user pass\r\n"));
    }

    #[test]
    fn untagged_exists_updates_mailbox_count() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        // SELECT response
        s.extend_from_slice(b"* 3 EXISTS\r\n");
        s.extend_from_slice(b"* 1 RECENT\r\n");
        s.extend_from_slice(b"* FLAGS (\\Seen \\Answered \\Deleted)\r\n");
        s.extend_from_slice(b"* OK [UIDVALIDITY 3857529045] UIDs valid\r\n");
        s.extend_from_slice(b"A0002 OK [READ-WRITE] SELECT completed\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        let mb = c.select(&mut t, "INBOX").unwrap();
        assert_eq!(mb.exists, Some(3));
        assert_eq!(mb.recent, Some(1));
        assert_eq!(mb.uid_validity, Some(3857529045));
        assert!(mb.flags.iter().any(|f| f == "\\Seen"));
        assert_eq!(c.state(), ImapState::Selected);
    }

    #[test]
    fn fetch_body_literal_is_read_by_exact_count_then_parsed() {
        // The critical literal test: BODY[] {21} + EXACTLY 21 octets, NOT
        // line-oriented. The literal contains two CRLFs (the header/body
        // separator) which MUST be read by count, not consumed as line breaks.
        let lit = b"Subject: Hi\r\n\r\nHello!"; // exactly 21 bytes, embedded CRLFs
        assert_eq!(lit.len(), 21);

        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n"); // minimal select
                                                          // FETCH with a literal
        s.extend_from_slice(b"* 1 FETCH (BODY[] {21}\r\n");
        s.extend_from_slice(lit); // raw 21 octets, INCLUDING two CRLFs
        s.extend_from_slice(b")\r\n"); // close the FETCH paren on continuation line
        s.extend_from_slice(b"A0003 OK FETCH completed\r\n");

        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        let results = c.fetch(&mut t, 1, "BODY[]").unwrap();
        assert_eq!(results.len(), 1);
        let body_bytes = results[0].body.as_ref().unwrap();
        // EXACTLY the 21 octets, including the embedded CRLFs — proves the read
        // was by count, not by newline (a line read would stop at the first \n).
        assert_eq!(body_bytes.len(), 21);
        assert_eq!(body_bytes.as_slice(), lit);
        // parsed via rae_mime-backed message parser
        let msg = results[0].parse_body().unwrap();
        assert_eq!(msg.envelope.subject, "Hi");
        assert_eq!(msg.body_text(), "Hello!");
    }

    #[test]
    fn fetch_envelope_extracts_from_and_subject() {
        let env = "(\"Mon, 1 Jan 2026 10:00:00 +0000\" \"Greetings\" \
((\"Ada\" NIL \"ada\" \"x.com\")) ((\"Ada\" NIL \"ada\" \"x.com\")) NIL \
((NIL NIL \"bob\" \"y.com\")) NIL NIL NIL \"<id@x.com>\")";
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(alloc::format!("* 5 FETCH (ENVELOPE {})\r\n", env).as_bytes());
        s.extend_from_slice(b"A0003 OK FETCH completed\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        let r = c.fetch(&mut t, 5, "ENVELOPE").unwrap();
        let e = r[0].envelope.as_ref().unwrap();
        assert_eq!(e.subject, "Greetings");
        assert_eq!(e.from[0].addr, "ada@x.com");
        assert_eq!(r[0].seq, 5);
    }

    #[test]
    fn fetch_flags_and_size() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(b"* 2 FETCH (FLAGS (\\Seen) RFC822.SIZE 4096 INTERNALDATE \"01-Jan-2026 10:00:00 +0000\")\r\n");
        s.extend_from_slice(b"A0003 OK done\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        let r = c
            .fetch(&mut t, 2, "(FLAGS RFC822.SIZE INTERNALDATE)")
            .unwrap();
        assert_eq!(r[0].flags, alloc::vec!["\\Seen".to_string()]);
        assert_eq!(r[0].rfc822_size, Some(4096));
        assert!(r[0].internal_date.contains("01-Jan-2026"));
    }

    #[test]
    fn list_parses_mailboxes() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n");
        s.extend_from_slice(b"* LIST (\\HasChildren) \"/\" \"Work\"\r\n");
        s.extend_from_slice(b"A0002 OK LIST completed\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        let boxes = c.list(&mut t, "", "*").unwrap();
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].name, "INBOX");
        assert_eq!(boxes[0].delimiter, '/');
        assert_eq!(boxes[1].name, "Work");
        assert!(boxes[1].attributes.iter().any(|a| a == "\\HasChildren"));
    }

    #[test]
    fn search_parses_ids() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(b"* SEARCH 2 84 882\r\n");
        s.extend_from_slice(b"A0003 OK SEARCH completed\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        let ids = c.search(&mut t, "UNSEEN").unwrap();
        assert_eq!(ids, alloc::vec![2, 84, 882]);
    }

    #[test]
    fn store_updates_flags() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(b"* 1 FETCH (FLAGS (\\Seen \\Deleted))\r\n");
        s.extend_from_slice(b"A0003 OK STORE completed\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        let upd = c.store(&mut t, 1, "+FLAGS", "(\\Deleted)").unwrap();
        assert!(t.sent_contains("A0003 STORE 1 +FLAGS (\\Deleted)\r\n"));
        assert!(upd[0].flags.iter().any(|f| f == "\\Deleted"));
    }

    #[test]
    fn tagged_no_and_bad_surface_as_errors() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 NO [AUTHENTICATIONFAILED] bad creds\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        match c.login(&mut t, "u", "wrong") {
            Err(ImapError::No(msg)) => assert!(msg.contains("bad creds")),
            other => panic!("expected No, got {:?}", other),
        }

        let mut s2 = Vec::new();
        s2.extend_from_slice(b"* OK ready\r\n");
        s2.extend_from_slice(b"A0001 BAD syntax error\r\n");
        let mut t2 = ScriptedTransport::new(&s2);
        let mut c2 = ImapClient::new();
        c2.read_greeting(&mut t2).unwrap();
        match c2.login(&mut t2, "u", "p") {
            Err(ImapError::Bad(_)) => {}
            other => panic!("expected Bad, got {:?}", other),
        }
    }

    // ---- hostile / never-panic / never-loop ----

    #[test]
    fn huge_literal_is_bounded_not_oom() {
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        // server advertises a 999999999-byte literal (~1GB) — must be rejected
        s.extend_from_slice(b"* 1 FETCH (BODY[] {999999999}\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        match c.fetch(&mut t, 1, "BODY[]") {
            Err(ImapError::LiteralTooLarge(_)) => {}
            other => panic!("expected LiteralTooLarge, got {:?}", other),
        }
    }

    #[test]
    fn literal_overflow_length_is_bounded() {
        // a length so long it overflows usize parsing → typed error, no panic
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(b"* 1 FETCH (BODY[] {999999999999999999999999999}\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        assert!(matches!(
            c.fetch(&mut t, 1, "BODY[]"),
            Err(ImapError::LiteralTooLarge(_))
        ));
    }

    #[test]
    fn truncated_mid_literal_is_graceful_err() {
        // announce {100} but only provide 5 bytes then EOF
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        s.extend_from_slice(b"A0002 OK SELECT done\r\n");
        s.extend_from_slice(b"* 1 FETCH (BODY[] {100}\r\nshort");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        c.select(&mut t, "INBOX").unwrap();
        assert!(matches!(
            c.fetch(&mut t, 1, "BODY[]"),
            Err(ImapError::Transport(TransportError::Closed))
        ));
    }

    #[test]
    fn server_never_sends_tag_is_bounded() {
        // floods untagged lines and never sends the tagged completion → EOF →
        // typed transport error, never an infinite loop.
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        for _ in 0..50 {
            s.extend_from_slice(b"* 1 EXISTS\r\n");
        }
        // then EOF (no A0002 completion)
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        match c.capability(&mut t) {
            Err(ImapError::Transport(TransportError::Closed)) => {}
            other => panic!("expected Closed, got {:?}", other),
        }
    }

    #[test]
    fn untagged_flood_is_bounded_by_max_untagged() {
        // Use a transport that yields "* x\r\n" effectively forever to prove the
        // MAX_UNTAGGED cap fires. We synthesize many lines (cheaper than infinite)
        // and a recv that loops: emulate by a large but finite flood > a small cap
        // is impractical (cap is 100k); instead prove the counter exists by
        // confirming a 200k-line flood returns TooManyResponses, not OOM/hang.
        let mut s = Vec::new();
        s.extend_from_slice(b"* OK ready\r\n");
        s.extend_from_slice(b"A0001 OK LOGIN done\r\n");
        for _ in 0..(crate::limits::MAX_UNTAGGED + 10) {
            s.extend_from_slice(b"* 1 EXISTS\r\n");
        }
        s.extend_from_slice(b"A0002 OK done\r\n");
        let mut t = ScriptedTransport::new(&s);
        let mut c = ImapClient::new();
        c.read_greeting(&mut t).unwrap();
        c.login(&mut t, "u", "p").unwrap();
        assert_eq!(c.capability(&mut t), Err(ImapError::TooManyResponses));
    }

    #[test]
    fn bad_greeting_surfaces() {
        let mut t = ScriptedTransport::new(b"* BYE server going down\r\n");
        let mut c = ImapClient::new();
        assert!(matches!(
            c.read_greeting(&mut t),
            Err(ImapError::BadGreeting(_))
        ));
    }

    #[test]
    fn literal_length_parser_unit() {
        assert_eq!(trailing_literal_len(b"foo {23}").unwrap(), Some(23));
        assert_eq!(trailing_literal_len(b"foo {0}").unwrap(), Some(0));
        assert_eq!(trailing_literal_len(b"foo {12+}").unwrap(), Some(12));
        assert_eq!(trailing_literal_len(b"no literal").unwrap(), None);
        // a '}' that isn't a literal
        assert_eq!(trailing_literal_len(b"text}").unwrap(), None);
        // empty braces
        assert!(trailing_literal_len(b"x {}").is_err());
    }

    /// Seeded fuzz over the response reader: random bytes terminated so recv_line
    /// completes → never panic, always a typed result.
    #[test]
    fn fuzz_response_parser_panic_free() {
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
        let mut rng = Rng(0xCAFE_F00D_1234_5678);
        for _ in 0..3000 {
            let len = (rng.b() as usize) % 160;
            let mut buf = Vec::new();
            buf.extend_from_slice(b"* OK ready\r\nA0001 OK LOGIN done\r\n");
            // a random untagged-ish line
            buf.extend_from_slice(b"* ");
            for _ in 0..len {
                let byte = rng.b();
                // keep CR/LF out of the random middle so we control termination,
                // but allow '{' '}' digits to exercise the literal path
                if byte == b'\r' || byte == b'\n' {
                    buf.push(b'.');
                } else {
                    buf.push(byte);
                }
            }
            buf.extend_from_slice(b"\r\n");
            buf.extend_from_slice(b"A0002 OK done\r\n");
            let mut t = ScriptedTransport::new(&buf);
            let mut c = ImapClient::new();
            c.read_greeting(&mut t).unwrap();
            c.login(&mut t, "u", "p").unwrap();
            // must return Ok/Err, never panic / hang
            let _ = c.capability(&mut t);
        }
    }
}
