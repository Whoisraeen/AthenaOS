//! AthenaOS Mail — *"the common apps people rely on, available and just work"*
//! (LEGACY_GAMING_CONCEPT.md charter app #5, "mail"). The macOS Mail / Windows Mail of
//! AthenaOS: a clickable mailbox + reading pane + compose flow over the LIVE,
//! already-host-KAT'd [`ath_mail`] protocol core.
//!
//! Standalone userspace ELF launched from the start menu (`exec_path = "mail"`).
//! Every byte of protocol work is done by the engines — this crate is the shell:
//!   * [`ath_mail::ImapClient`] / [`ath_mail::Pop3Client`] — fetch the message
//!     list + raw bodies over an INJECTABLE [`ath_mail::MailTransport`] byte pipe.
//!   * [`ath_mail::FetchedMessage::parse`] — the never-panic RFC 822 / 2045 parse
//!     that turns a fetched `BODY[]` / `RETR` payload into headers + decoded parts
//!     for the reading pane (multipart → pick text/plain, else stripped first part).
//!   * [`ath_mail::SmtpClient`] / [`ath_mail::OutgoingMessage`] — the compose flow
//!     builds an RFC 822 message and runs the SMTP send dialog over the same
//!     transport seam.
//!   * [`ath_pim::parse_vcf`] — vCard import feeds the compose To: autocomplete.
//!   * [`ath_kv::KvStore`] — the local mailbox cache (one entry per message),
//!     account settings, and drafts persist through its versioned, CRC-checked
//!     snapshot.
//!
//! ## The transport seam (the whole reason this is host-provable)
//! Every network op takes a `&mut impl MailTransport`. In the host KAT
//! (`cargo test -p mail --features host`) that is a [`MockTransport`] replaying a
//! scripted IMAP/POP3/SMTP server dialog with ZERO network — so the message-list
//! populate, the multipart reading-pane parse, and the compose-send-DATA contents
//! are all asserted on the dev box. In the live ELF (`cfg(not(test))`) it is a
//! [`SocketTransport`] over the kernel's userspace net syscalls (121-125) — no
//! `athnet` dependency, so the host build never pulls a socket stack (and never
//! trips the poly1305 SIMD bare-build issue). Real send/receive is therefore
//! iron-gated on networking; the host proof needs no network.
//!
//! ## Never panics on hostile input
//! A mail server is untrusted: `ath_mail` bounds every line/literal/response and
//! `FetchedMessage::parse` returns `Err` (never panics, never loops) on truncated
//! or garbage bytes. The reading pane surfaces a parse failure as a message, never
//! a crash — proven by the hostile-bytes KAT.

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, and the live
// `SocketTransport` issues raw net syscalls, so the LIBRARY cannot
// `#![forbid(unsafe_code)]` — only the documented sites are unsafe.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use ath_kv::KvStore;
use ath_mail::{
    FetchedMessage, ImapClient, ImapState, MailTransport, OutgoingMessage, Pop3Client, SmtpClient,
};
use ath_pim::parse_vcf;

// The render/run path is live-ELF only; under `cargo test` only the MailModel
// (over the engines) is exercised, so the graphics/syscall imports are gated out
// to keep the host test warning-clean.
#[cfg(not(test))]
#[allow(unused_imports)]
use athkit;

#[cfg(not(test))]
use ath_tokens::DARK;
#[cfg(not(test))]
use athgfx::text::FontFamily;
#[cfg(not(test))]
use athgfx::Canvas;

// ── Window geometry (live ELF only) ──────────────────────────────────────

#[cfg(not(test))]
const WIN_W: usize = 760;
#[cfg(not(test))]
const WIN_H: usize = 560;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7D00_0000;

#[cfg(not(test))]
const TITLE_H: usize = 28;
#[cfg(not(test))]
const TOOLBAR_H: usize = 34;
#[cfg(not(test))]
const FOOTER_H: usize = 30;
/// Width of the left folder rail.
#[cfg(not(test))]
const RAIL_W: usize = 150;
/// Width of the message list column.
#[cfg(not(test))]
const LIST_W: usize = 250;

/// On-screen present origin.
#[cfg(not(test))]
const PRESENT_X: i32 = 120;
#[cfg(not(test))]
const PRESENT_Y: i32 = 50;

/// Max messages we list/cache from one fetch — a hard cap so a server claiming a
/// huge mailbox can never flood the list or the KV cache. (`ath_mail` itself
/// bounds each line/literal; this bounds how many we walk.)
const MAX_MESSAGES: u32 = 200;

/// Max raw message bytes we cache per message in KV (a reading-pane message, not
/// an archive). `ath_mail::limits::MAX_LITERAL` is the protocol cap; this is the
/// per-entry storage cap.
const MAX_CACHED_BODY: usize = 1024 * 1024;

// ── KV cache keys ────────────────────────────────────────────────────────
//
// ath_kv is an ORDERED store, so message bodies key on a zero-padded sequence so
// a prefix scan returns them in mailbox order. Settings + drafts use stable keys.

/// Key for a cached raw message body: `msg/<5-digit seq>`.
fn body_key(seq: u32) -> Vec<u8> {
    format!("msg/{:05}", seq).into_bytes()
}
/// Key for the cached one-line summary of a message: `sum/<5-digit seq>`.
fn summary_key(seq: u32) -> Vec<u8> {
    format!("sum/{:05}", seq).into_bytes()
}
const KEY_ACCOUNT_HOST: &[u8] = b"acct/host";
const KEY_ACCOUNT_USER: &[u8] = b"acct/user";
const KEY_DRAFT_TO: &[u8] = b"draft/to";
const KEY_DRAFT_SUBJECT: &[u8] = b"draft/subject";
const KEY_DRAFT_BODY: &[u8] = b"draft/body";

// ===========================================================================
// MailModel — the syscall-free heart (host-KAT'd against the live engines).
// ===========================================================================

/// One row in the message list: the engine-fetched envelope summary, plus the
/// IMAP/POP3 sequence number that addresses the raw body in the KV cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageSummary {
    /// The IMAP sequence number / POP3 message number.
    pub seq: u32,
    pub from: String,
    pub subject: String,
    pub date: String,
    /// True once the body has been fetched + cached (so it can be opened offline).
    pub cached: bool,
    /// True if the \Seen flag was present in the fetch.
    pub seen: bool,
}

/// The account settings the app persists in KV. The password is NOT stored
/// (entered per-session / supplied to the live transport); only the non-secret
/// connection identity is cached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub host: String,
    pub user: String,
}

/// A compose draft (the editable fields the compose pane binds to).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// The result of opening a message into the reading pane: the parsed headers we
/// show + the chosen displayable body text. A hostile/truncated message yields
/// [`OpenedMessage`] with an error note in `body` and empty headers — never a
/// panic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenedMessage {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    /// The displayable body (text/plain part, else stripped first part).
    pub body: String,
    /// Number of decoded MIME parts (1 for a simple message).
    pub part_count: usize,
    /// True when the raw bytes failed to parse (body holds the explanation).
    pub parse_failed: bool,
}

/// The in-memory mail state over the LIVE engines. All decision/query logic
/// (fetch → summarize, cache round-trip, open → parse, compose → build) is here
/// and syscall-free, so the host KAT drives it directly against a mock transport.
pub struct MailModel {
    /// The local cache + settings store (the LIVE ath_kv).
    kv: KvStore,
    /// The current message list (newest sequence first after a fetch).
    messages: Vec<MessageSummary>,
    /// The active account (host/user), loaded from / saved to KV.
    account: Account,
    /// Contacts imported from a vCard, for compose To: autocomplete.
    contacts: Vec<(String, String)>, // (display name, email)
    /// The current compose draft.
    draft: Draft,
}

impl Default for MailModel {
    fn default() -> Self {
        Self::new()
    }
}

impl MailModel {
    /// An empty model with a fresh in-memory KV cache.
    pub fn new() -> MailModel {
        MailModel {
            kv: KvStore::new(),
            messages: Vec::new(),
            account: Account::default(),
            contacts: Vec::new(),
            draft: Draft::default(),
        }
    }

    /// Reconstruct a model from a previously-serialized KV snapshot (the on-disk
    /// cache). A corrupt/truncated blob yields a fresh empty cache rather than an
    /// error — the cache is best-effort. Re-loads the account + message summaries.
    pub fn from_cache_bytes(bytes: &[u8]) -> MailModel {
        let kv = KvStore::from_bytes(bytes).unwrap_or_else(|_| KvStore::new());
        let mut m = MailModel {
            kv,
            messages: Vec::new(),
            account: Account::default(),
            contacts: Vec::new(),
            draft: Draft::default(),
        };
        m.reload_from_kv();
        m
    }

    /// Snapshot the whole cache (account + summaries + bodies + draft) to bytes for
    /// persistence. Round-trips through [`from_cache_bytes`].
    pub fn to_cache_bytes(&self) -> Vec<u8> {
        self.kv.to_bytes()
    }

    /// Re-derive the in-memory account + message list from the KV store (used after
    /// loading a snapshot).
    fn reload_from_kv(&mut self) {
        self.account = Account {
            host: self.kv.get_str(KEY_ACCOUNT_HOST).unwrap_or("").to_string(),
            user: self.kv.get_str(KEY_ACCOUNT_USER).unwrap_or("").to_string(),
        };
        self.draft = Draft {
            to: self.kv.get_str(KEY_DRAFT_TO).unwrap_or("").to_string(),
            subject: self.kv.get_str(KEY_DRAFT_SUBJECT).unwrap_or("").to_string(),
            body: self.kv.get_str(KEY_DRAFT_BODY).unwrap_or("").to_string(),
        };
        // Rebuild the message list from the cached summaries (ordered by key).
        let mut msgs: Vec<MessageSummary> = Vec::new();
        let pairs: Vec<(Vec<u8>, Vec<u8>)> = self
            .kv
            .prefix_scan(b"sum/")
            .map(|(k, v)| (k.to_vec(), v.to_vec()))
            .collect();
        for (k, v) in pairs {
            // key is "sum/NNNNN"; the seq is the suffix.
            let seq = core::str::from_utf8(&k[4..])
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            if let Some(mut s) = decode_summary(seq, &v) {
                s.cached = self.kv.contains(body_key(seq));
                msgs.push(s);
            }
        }
        // Newest-first (highest seq at the top), the macOS Mail ordering.
        msgs.sort_by(|a, b| b.seq.cmp(&a.seq));
        self.messages = msgs;
    }

    /// The active account (host/user identity).
    pub fn account(&self) -> &Account {
        &self.account
    }

    /// Set + persist the account identity (non-secret connection info).
    pub fn set_account(&mut self, host: &str, user: &str) {
        self.account = Account {
            host: host.to_string(),
            user: user.to_string(),
        };
        let _ = self.kv.put_str(KEY_ACCOUNT_HOST.to_vec(), host);
        let _ = self.kv.put_str(KEY_ACCOUNT_USER.to_vec(), user);
    }

    /// The current message list (newest first).
    pub fn messages(&self) -> &[MessageSummary] {
        &self.messages
    }

    /// Number of messages in the current list.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// The current compose draft.
    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// Mutable draft for editing; persists on [`save_draft`].
    pub fn draft_mut(&mut self) -> &mut Draft {
        &mut self.draft
    }

    /// Persist the current draft to the KV cache.
    pub fn save_draft(&mut self) {
        let _ = self.kv.put_str(KEY_DRAFT_TO.to_vec(), &self.draft.to);
        let _ = self
            .kv
            .put_str(KEY_DRAFT_SUBJECT.to_vec(), &self.draft.subject);
        let _ = self.kv.put_str(KEY_DRAFT_BODY.to_vec(), &self.draft.body);
    }

    /// Import contacts from a vCard (`.vcf`) document for compose autocomplete.
    /// Returns the number of (name,email) pairs harvested. A parse failure leaves
    /// the existing contacts intact and returns 0.
    pub fn import_contacts(&mut self, vcf: &str) -> usize {
        let book = match parse_vcf(vcf) {
            Ok(b) => b,
            Err(_) => return 0,
        };
        let mut out: Vec<(String, String)> = Vec::new();
        for c in &book.contacts {
            // Only contacts with at least one email are useful for compose.
            if let Some(first) = c.emails.first() {
                let name = if !c.fn_name.is_empty() {
                    c.fn_name.clone()
                } else {
                    first.value.clone()
                };
                out.push((name, first.value.clone()));
            }
        }
        let n = out.len();
        self.contacts = out;
        n
    }

    /// Compose autocomplete: contacts whose name OR email contains `query`
    /// (case-insensitive). Empty query returns all. Bounded to 8 suggestions.
    pub fn autocomplete(&self, query: &str) -> Vec<(String, String)> {
        let q = query.to_ascii_lowercase();
        self.contacts
            .iter()
            .filter(|(name, email)| {
                q.is_empty()
                    || name.to_ascii_lowercase().contains(&q)
                    || email.to_ascii_lowercase().contains(&q)
            })
            .take(8)
            .cloned()
            .collect()
    }

    // ── IMAP fetch path ───────────────────────────────────────────────────

    /// Fetch the message list from an IMAP server over the injected transport:
    /// greeting → LOGIN → SELECT mailbox → FETCH envelope+flags for each message,
    /// caching each summary in KV. Replaces the current list. Returns the number
    /// of messages listed, or the [`ath_mail::ImapError`] (the existing cache is
    /// left intact on failure).
    ///
    /// Bodies are fetched lazily by [`fetch_imap_body`] when a message is opened —
    /// the list fetch only pulls envelopes (the macOS Mail "headers first" model).
    pub fn fetch_imap<T: MailTransport>(
        &mut self,
        t: &mut T,
        mailbox: &str,
        user: &str,
        pass: &str,
    ) -> Result<usize, ath_mail::ImapError> {
        let mut client = ImapClient::new();
        client.read_greeting(t)?;
        if client.state() == ImapState::NotAuthenticated {
            client.login(t, user, pass)?;
        }
        let mbox = client.select(t, mailbox)?;
        let exists = mbox.exists.unwrap_or(0).min(MAX_MESSAGES);

        let mut listed: Vec<MessageSummary> = Vec::new();
        for seq in 1..=exists {
            let results = client.fetch(t, seq, "(FLAGS ENVELOPE)")?;
            for fr in results {
                let env = fr.envelope.unwrap_or_default();
                let from = env
                    .from
                    .first()
                    .map(addr_display)
                    .unwrap_or_else(|| "(unknown)".to_string());
                let seen = fr.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"));
                let summary = MessageSummary {
                    seq: fr.seq.max(seq),
                    from,
                    subject: if env.subject.is_empty() {
                        "(no subject)".to_string()
                    } else {
                        env.subject.clone()
                    },
                    date: env.date.clone(),
                    cached: self.kv.contains(body_key(fr.seq.max(seq))),
                    seen,
                };
                let _ = self
                    .kv
                    .put(summary_key(summary.seq), encode_summary(&summary));
                listed.push(summary);
            }
        }
        let _ = client.logout(t);
        listed.sort_by(|a, b| b.seq.cmp(&a.seq));
        let n = listed.len();
        self.messages = listed;
        Ok(n)
    }

    /// Fetch + cache one message's raw body over IMAP (`BODY[]`), returning the raw
    /// bytes. Caches under [`body_key`] so a later open is offline. Bounded by
    /// [`MAX_CACHED_BODY`].
    pub fn fetch_imap_body<T: MailTransport>(
        &mut self,
        t: &mut T,
        mailbox: &str,
        user: &str,
        pass: &str,
        seq: u32,
    ) -> Result<Vec<u8>, ath_mail::ImapError> {
        let mut client = ImapClient::new();
        client.read_greeting(t)?;
        if client.state() == ImapState::NotAuthenticated {
            client.login(t, user, pass)?;
        }
        client.select(t, mailbox)?;
        let results = client.fetch(t, seq, "(BODY[])")?;
        let body = results
            .iter()
            .find_map(|r| r.body.clone())
            .unwrap_or_default();
        let _ = client.logout(t);
        self.cache_body(seq, &body);
        Ok(body)
    }

    // ── POP3 fetch path ─────────────────────────────────────────────────────

    /// Fetch the message list from a POP3 server: greeting → USER → PASS → STAT →
    /// RETR each message (POP3 has no envelope fetch, so we RETR + parse to get the
    /// summary, caching the raw body as we go). Replaces the current list. Returns
    /// the count, or the [`ath_mail::Pop3Error`] (cache intact on failure).
    pub fn fetch_pop3<T: MailTransport>(
        &mut self,
        t: &mut T,
        user: &str,
        pass: &str,
    ) -> Result<usize, ath_mail::Pop3Error> {
        let mut client = Pop3Client::new();
        client.read_greeting(t)?;
        client.user(t, user)?;
        client.pass(t, pass)?;
        let stat = client.stat(t)?;
        let count = stat.count.min(MAX_MESSAGES);

        let mut listed: Vec<MessageSummary> = Vec::new();
        for n in 1..=count {
            let raw = client.retr(t, n)?;
            // Parse the headers for the summary; on failure list a placeholder.
            let summary = match FetchedMessage::parse(&raw) {
                Ok(msg) => MessageSummary {
                    seq: n,
                    from: msg
                        .envelope
                        .from
                        .first()
                        .map(addr_display)
                        .unwrap_or_else(|| "(unknown)".to_string()),
                    subject: if msg.envelope.subject.is_empty() {
                        "(no subject)".to_string()
                    } else {
                        msg.envelope.subject.clone()
                    },
                    date: msg.envelope.date.clone(),
                    cached: true,
                    seen: false,
                },
                Err(_) => MessageSummary {
                    seq: n,
                    from: "(unparseable)".to_string(),
                    subject: "(malformed message)".to_string(),
                    date: String::new(),
                    cached: true,
                    seen: false,
                },
            };
            self.cache_body(n, &raw);
            let _ = self.kv.put(summary_key(n), encode_summary(&summary));
            listed.push(summary);
        }
        let _ = client.quit(t);
        listed.sort_by(|a, b| b.seq.cmp(&a.seq));
        let n = listed.len();
        self.messages = listed;
        Ok(n)
    }

    /// Cache one raw message body in KV (bounded), marking its summary cached.
    fn cache_body(&mut self, seq: u32, raw: &[u8]) {
        if raw.is_empty() {
            return;
        }
        let bytes = if raw.len() > MAX_CACHED_BODY {
            &raw[..MAX_CACHED_BODY]
        } else {
            raw
        };
        let _ = self.kv.put(body_key(seq), bytes.to_vec());
        if let Some(m) = self.messages.iter_mut().find(|m| m.seq == seq) {
            m.cached = true;
        }
    }

    // ── Reading pane ─────────────────────────────────────────────────────────

    /// Open the cached message at sequence `seq` into the reading pane: pull the
    /// raw bytes from the KV cache, parse via the LIVE [`FetchedMessage::parse`],
    /// and project headers + a displayable body. Returns `None` if no body is
    /// cached for that seq; returns an [`OpenedMessage`] with `parse_failed=true`
    /// (and an explanatory body) on hostile/truncated bytes — never panics.
    pub fn open(&self, seq: u32) -> Option<OpenedMessage> {
        let raw = self.kv.get(body_key(seq))?;
        Some(open_raw(raw))
    }

    /// Open arbitrary raw message bytes (used by the host KAT to drive the parser
    /// directly). Never panics on hostile input.
    pub fn open_bytes(raw: &[u8]) -> OpenedMessage {
        open_raw(raw)
    }

    // ── Compose / send ───────────────────────────────────────────────────────

    /// Build an RFC 822 message from a draft and send it over the injected SMTP
    /// transport: greeting → EHLO → (AUTH if user/pass) → MAIL FROM → RCPT TO →
    /// DATA. Returns Ok on a fully accepted send, else the [`ath_mail::SmtpError`].
    /// On success the draft is cleared + the cleared draft persisted.
    pub fn send<T: MailTransport>(
        &mut self,
        t: &mut T,
        from: &str,
        user: &str,
        pass: &str,
        domain: &str,
    ) -> Result<(), ath_mail::SmtpError> {
        let msg = build_outgoing(from, &self.draft);
        let mut client = SmtpClient::new();
        client.read_greeting(t)?;
        client.ehlo(t, domain)?;
        if !user.is_empty() {
            // Prefer AUTH PLAIN where advertised; fall back to LOGIN.
            if client.caps().auth_plain {
                client.auth_plain(t, user, pass)?;
            } else if client.caps().auth_login {
                client.auth_login(t, user, pass)?;
            }
        }
        client.send_message(t, &msg)?;
        let _ = client.quit(t);
        // Clear the draft on a successful send.
        self.draft = Draft::default();
        self.save_draft();
        Ok(())
    }

    /// Build the RFC 822 message the current draft would send (without sending).
    /// Exposed so the host KAT can assert the serialized DATA contents.
    pub fn compose_message(&self, from: &str) -> OutgoingMessage {
        build_outgoing(from, &self.draft)
    }
}

// ── Free helpers (syscall-free, host-tested via MailModel) ──────────────────

/// Render an [`ath_mail::Address`] as a display string: `Name <addr>` if a name is
/// present, else the bare address.
fn addr_display(a: &ath_mail::Address) -> String {
    if a.name.is_empty() {
        a.addr.clone()
    } else {
        format!("{} <{}>", a.name, a.addr)
    }
}

/// Project a parsed [`FetchedMessage`] into the reading-pane [`OpenedMessage`].
fn project(msg: &FetchedMessage) -> OpenedMessage {
    let from = msg
        .envelope
        .from
        .first()
        .map(addr_display)
        .unwrap_or_default();
    let to = msg
        .envelope
        .to
        .iter()
        .map(addr_display)
        .collect::<Vec<_>>()
        .join(", ");
    OpenedMessage {
        from,
        to,
        subject: msg.envelope.subject.clone(),
        date: msg.envelope.date.clone(),
        body: msg.body_text(),
        part_count: msg.parts.len(),
        parse_failed: false,
    }
}

/// Parse + project raw message bytes; degrade cleanly (never panic) on bad bytes.
fn open_raw(raw: &[u8]) -> OpenedMessage {
    match FetchedMessage::parse(raw) {
        Ok(msg) => project(&msg),
        Err(e) => OpenedMessage {
            from: String::new(),
            to: String::new(),
            subject: "(could not display this message)".to_string(),
            date: String::new(),
            body: format!("This message could not be parsed: {:?}", e),
            part_count: 0,
            parse_failed: true,
        },
    }
}

/// Build an [`OutgoingMessage`] from a draft. Recipients are comma/semicolon
/// separated and trimmed; an empty subject/body is allowed (the server decides).
fn build_outgoing(from: &str, draft: &Draft) -> OutgoingMessage {
    let mut msg = OutgoingMessage::new(from, "", &draft.subject, &draft.body);
    msg.to = draft
        .to
        .split(|c| c == ',' || c == ';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    msg
}

/// Encode a [`MessageSummary`] for the KV cache: a `\x1f`-delimited record
/// (from, subject, date, seen). The seq is the key, not stored.
fn encode_summary(s: &MessageSummary) -> Vec<u8> {
    let seen = if s.seen { "1" } else { "0" };
    // Replace any stray unit separators in fields so the split round-trips.
    let scrub = |x: &str| -> String { x.replace('\u{1f}', " ") };
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        scrub(&s.from),
        scrub(&s.subject),
        scrub(&s.date),
        seen
    )
    .into_bytes()
}

/// Decode a cached summary record. Returns `None` on a malformed record (never
/// panics).
fn decode_summary(seq: u32, bytes: &[u8]) -> Option<MessageSummary> {
    let s = core::str::from_utf8(bytes).ok()?;
    let mut it = s.split('\u{1f}');
    let from = it.next()?.to_string();
    let subject = it.next()?.to_string();
    let date = it.next()?.to_string();
    let seen = it.next().map(|x| x == "1").unwrap_or(false);
    Some(MessageSummary {
        seq,
        from,
        subject,
        date,
        cached: false,
        seen,
    })
}

// ===========================================================================
// App state + render (live ELF only — syscall-touching).
// ===========================================================================

/// Which top-level view is showing.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    /// Folder rail + message list + reading pane.
    Inbox,
    /// The compose form.
    Compose,
}

/// A short status line shown in the footer.
#[cfg(not(test))]
struct Toast {
    text: String,
}

#[cfg(not(test))]
impl Toast {
    fn new() -> Toast {
        Toast {
            text: String::new(),
        }
    }
    fn set(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }
    fn clear(&mut self) {
        self.text.clear();
    }
}

/// The whole live app.
#[cfg(not(test))]
struct App {
    model: MailModel,
    view: View,
    /// Selected message-list row index.
    sel: usize,
    /// The opened message in the reading pane, if any.
    open: Option<OpenedMessage>,
    toast: Toast,
}

#[cfg(not(test))]
impl App {
    fn new() -> App {
        // Load any persisted cache from the session home; else start empty.
        let model = match read_home_bytes("mail.cache") {
            Some(bytes) => MailModel::from_cache_bytes(&bytes),
            None => MailModel::new(),
        };
        let mut app = App {
            model,
            view: View::Inbox,
            sel: 0,
            open: None,
            toast: Toast::new(),
        };
        // Seed compose autocomplete from the contacts export the Calendar app uses.
        if let Some(vcf) = read_home_string("import.vcf") {
            let _ = app.model.import_contacts(&vcf);
        }
        // Load the account identity from ~/mail.account ("host\nuser"), if present
        // and not already cached. Keeps the non-secret connection info; the
        // password lives in ~/mail.pass (read only at fetch time).
        if app.model.account().host.is_empty() {
            if let Some(txt) = read_home_string("mail.account") {
                let mut lines = txt.lines();
                let host = lines.next().unwrap_or("").trim();
                let user = lines.next().unwrap_or("").trim();
                if !host.is_empty() {
                    app.model.set_account(host, user);
                }
            }
        }
        if app.model.message_count() == 0 {
            app.toast
                .set("No cached mail. Connect an account to fetch.");
        }
        app
    }

    /// Open the currently-selected message into the reading pane.
    fn open_selected(&mut self) {
        if let Some(msg) = self.model.messages().get(self.sel) {
            let seq = msg.seq;
            self.open = self.model.open(seq);
            if self.open.is_none() {
                self.toast
                    .set("Message body not cached (fetch needed — iron-gated on net).");
            } else {
                self.toast.clear();
            }
        }
    }

    /// Connect to the configured account's IMAP server (port 143, clear-text —
    /// STARTTLS is the iron-gated follow-up) and fetch the INBOX list over the LIVE
    /// [`SocketTransport`]. This is the real wiring the host KAT proves against a
    /// mock; on iron it rides the kernel net syscalls. Without networking it fails
    /// gracefully with a toast (never panics). The password is read from
    /// `~/mail.pass` (a per-session secret kept out of the cache).
    fn fetch_now(&mut self) {
        let host = self.model.account().host.clone();
        let user = self.model.account().user.clone();
        if host.is_empty() || user.is_empty() {
            self.toast
                .set("Set an account first (~/mail.account: host\\nuser).");
            return;
        }
        let pass = read_home_string("mail.pass").unwrap_or_default();
        match SocketTransport::connect(&host, 143) {
            Some(mut t) => match self.model.fetch_imap(&mut t, "INBOX", &user, &pass) {
                Ok(n) => {
                    self.sel = 0;
                    self.open = None;
                    let mut s = String::from("Fetched ");
                    push_num(&mut s, n as u64);
                    s.push_str(" messages.");
                    self.toast.set(&s);
                    self.persist();
                }
                Err(_) => self.toast.set("Fetch failed (server error)."),
            },
            None => self
                .toast
                .set("Could not connect (networking is iron-gated)."),
        }
    }

    /// Persist the KV cache back to the session home (best-effort).
    fn persist(&self) {
        let bytes = self.model.to_cache_bytes();
        write_home_bytes("mail.cache", &bytes);
    }
}

/// Append a decimal number to `s` (no_std-safe).
#[cfg(not(test))]
fn push_num(s: &mut String, mut n: u64) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if let Ok(t) = core::str::from_utf8(&buf[i..]) {
        s.push_str(t);
    }
}

// ── Persistence / import (live ELF only) ────────────────────────────────────

/// Resolve `<session-home>/<name>` (or `/home/user/<name>`).
#[cfg(not(test))]
fn home_path(name: &str) -> String {
    let mut path = String::new();
    let mut info = [0u8; 96];
    if athkit::sys::session_info(&mut info).is_some() {
        if let Some(home) = athkit::sys::session_home_from(&info) {
            path.push_str(home);
            path.push('/');
            path.push_str(name);
        }
    }
    if path.is_empty() {
        path.push_str("/home/user/");
        path.push_str(name);
    }
    path
}

/// Read a session-home file into raw bytes, or `None`.
#[cfg(not(test))]
fn read_home_bytes(name: &str) -> Option<Vec<u8>> {
    let path = home_path(name);
    let fd = athkit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if data.len() > 16 * 1024 * 1024 {
            break;
        }
        let n = athkit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = athkit::sys::close(fd);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// Read a session-home file as a lossy-UTF-8 String, or `None`.
#[cfg(not(test))]
fn read_home_string(name: &str) -> Option<String> {
    read_home_bytes(name).map(lossy_string)
}

/// Write raw bytes to a session-home file (best-effort: a failure is silent — the
/// cache is non-critical). Flags 0x1|0x2|0x40 = O_WRONLY|O_TRUNC|O_CREAT.
#[cfg(not(test))]
fn write_home_bytes(name: &str, bytes: &[u8]) {
    let path = home_path(name);
    // O_WRONLY (1) | O_CREAT (0x40) | O_TRUNC (0x200) — the VFS open flag layout.
    let fd = athkit::sys::open(path.as_str(), 1 | 0x40 | 0x200);
    if fd == u64::MAX {
        return;
    }
    let mut off = 0;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = unsafe {
            athkit::sys::syscall3(
                athkit::sys::SYS_WRITE,
                fd,
                bytes[off..end].as_ptr() as u64,
                (end - off) as u64,
            )
        } as usize;
        if n == 0 || n > (end - off) {
            break;
        }
        off += n;
    }
    let _ = athkit::sys::close(fd);
}

/// Lossy UTF-8 decode of an owned byte vector (no_std-safe).
#[cfg(not(test))]
fn lossy_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            let mut out = String::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                match core::str::from_utf8(&bytes[i..]) {
                    Ok(valid) => {
                        out.push_str(valid);
                        break;
                    }
                    Err(e2) => {
                        let good = e2.valid_up_to();
                        if good > 0 {
                            if let Ok(s) = core::str::from_utf8(&bytes[i..i + good]) {
                                out.push_str(s);
                            }
                        }
                        out.push('\u{FFFD}');
                        i += good + 1;
                    }
                }
            }
            out
        }
    }
}

// ── Live transport over the kernel net syscalls (cfg(not(test)) ONLY) ────────
//
// The live ELF wraps the userspace net syscalls (121-125) in a MailTransport so
// the SAME engine dialogs that the host KAT proves against MockTransport run over
// a real socket on iron. NO `athnet` dependency — the kernel owns the TCP stack;
// this is a thin syscall shim. (TLS is NOT yet wired here: this connects in the
// clear, suitable for a local/test server; a STARTTLS upgrade is the documented
// iron-gated follow-up coordinated with athnet.) Real send/receive is therefore
// iron-gated on networking — the host proof never links this.

/// A clear-text TCP transport over the kernel net syscalls. Reads are buffered so
/// `recv_line` can return one CRLF-terminated line at a time (what the engines
/// expect) without a syscall per byte.
#[cfg(not(test))]
struct SocketTransport {
    fd: u64,
    buf: Vec<u8>,
    closed: bool,
}

#[cfg(not(test))]
impl SocketTransport {
    /// Open a TCP connection to `host:port`. `host` may be a dotted-quad or a name
    /// (resolved via SYS_NET_DNS). Returns `None` if the socket/connect fails.
    fn connect(host: &str, port: u16) -> Option<SocketTransport> {
        let ip = resolve_ip(host)?;
        let fd = unsafe { athkit::sys::syscall1(athkit::sys::SYS_NET_SOCKET, 0) };
        if fd == u64::MAX {
            return None;
        }
        let r = unsafe {
            athkit::sys::syscall3(athkit::sys::SYS_NET_CONNECT, fd, ip as u64, port as u64)
        };
        if r == u64::MAX {
            let _ = unsafe { athkit::sys::syscall1(athkit::sys::SYS_NET_CLOSE, fd) };
            return None;
        }
        Some(SocketTransport {
            fd,
            buf: Vec::new(),
            closed: false,
        })
    }

    /// Pull more bytes into the buffer; returns false on close/error.
    fn fill(&mut self) -> bool {
        if self.closed {
            return false;
        }
        let mut chunk = [0u8; 2048];
        let n = unsafe {
            athkit::sys::syscall3(
                athkit::sys::SYS_NET_RECV,
                self.fd,
                chunk.as_mut_ptr() as u64,
                chunk.len() as u64,
            )
        };
        if n == u64::MAX {
            self.closed = true;
            return false;
        }
        if n == 0 {
            // No data right now; yield and let the caller's bounded loop retry.
            athkit::sys::yield_now();
            return true;
        }
        let n = (n as usize).min(chunk.len());
        self.buf.extend_from_slice(&chunk[..n]);
        true
    }
}

#[cfg(not(test))]
impl Drop for SocketTransport {
    fn drop(&mut self) {
        let _ = unsafe { athkit::sys::syscall1(athkit::sys::SYS_NET_CLOSE, self.fd) };
    }
}

#[cfg(not(test))]
impl MailTransport for SocketTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), ath_mail::TransportError> {
        let mut off = 0;
        while off < data.len() {
            let n = unsafe {
                athkit::sys::syscall3(
                    athkit::sys::SYS_NET_SEND,
                    self.fd,
                    data[off..].as_ptr() as u64,
                    (data.len() - off) as u64,
                )
            };
            if n == u64::MAX || n == 0 {
                return Err(ath_mail::TransportError::Closed);
            }
            off += (n as usize).min(data.len() - off);
        }
        Ok(())
    }

    fn recv_line(&mut self) -> Result<Vec<u8>, ath_mail::TransportError> {
        // Bounded: never buffer past MAX_LINE, never loop forever.
        let mut spins = 0usize;
        loop {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                return Ok(line);
            }
            if self.buf.len() > ath_mail::limits::MAX_LINE {
                return Err(ath_mail::TransportError::Io("line too long".into()));
            }
            if !self.fill() {
                return Err(ath_mail::TransportError::Closed);
            }
            spins += 1;
            if spins > 100_000 {
                return Err(ath_mail::TransportError::Timeout);
            }
        }
    }

    fn recv_exact(&mut self, n: usize) -> Result<Vec<u8>, ath_mail::TransportError> {
        let mut spins = 0usize;
        while self.buf.len() < n {
            if !self.fill() {
                return Err(ath_mail::TransportError::Closed);
            }
            spins += 1;
            if spins > 1_000_000 {
                return Err(ath_mail::TransportError::Timeout);
            }
        }
        Ok(self.buf.drain(..n).collect())
    }
}

/// Resolve a host string to a packed BE u32 IPv4. Accepts dotted-quad directly or
/// falls back to SYS_NET_DNS.
#[cfg(not(test))]
fn resolve_ip(host: &str) -> Option<u32> {
    if let Some(ip) = parse_dotted_quad(host) {
        return Some(ip);
    }
    let r = unsafe {
        athkit::sys::syscall2(
            athkit::sys::SYS_NET_DNS,
            host.as_ptr() as u64,
            host.len() as u64,
        )
    };
    if r == u64::MAX {
        None
    } else {
        Some(r as u32)
    }
}

/// Parse `a.b.c.d` into a packed BE u32 (octet[0] in the high byte), or `None`.
#[cfg(not(test))]
fn parse_dotted_quad(s: &str) -> Option<u32> {
    let mut octets = [0u32; 4];
    let mut i = 0;
    for part in s.split('.') {
        if i >= 4 {
            return None;
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        octets[i] = v;
        i += 1;
    }
    if i != 4 {
        return None;
    }
    Some((octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3])
}

// ── Theme (live ELF only) ────────────────────────────────────────────────

#[cfg(not(test))]
const BG: u32 = DARK.bg_base;
#[cfg(not(test))]
const PANEL: u32 = DARK.bg_raised;
#[cfg(not(test))]
const RAIL_BG: u32 = DARK.bg_overlay;
#[cfg(not(test))]
const ROW_BG: u32 = DARK.bg_overlay;
#[cfg(not(test))]
const ROW_SEL: u32 = DARK.bg_elevated;
#[cfg(not(test))]
const STROKE: u32 = DARK.stroke_subtle;
#[cfg(not(test))]
const TEXT_PRIMARY: u32 = DARK.text_primary;
#[cfg(not(test))]
const TEXT_SECONDARY: u32 = DARK.text_secondary;
#[cfg(not(test))]
const TEXT_TERTIARY: u32 = DARK.text_tertiary;

#[cfg(not(test))]
fn accent() -> u32 {
    ath_tokens::derive_accent(athkit::sys::theme_accent(), &DARK).base
}

// ── Render ──────────────────────────────────────────────────────────────────

#[cfg(not(test))]
fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar.
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, PANEL);
    canvas.draw_text_aa(
        10,
        ((TITLE_H - ath_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Mail",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    render_toolbar(app, canvas);
    match app.view {
        View::Inbox => render_inbox(app, canvas),
        View::Compose => render_compose(app, canvas),
    }
    render_footer(app, canvas);
}

#[cfg(not(test))]
fn render_toolbar(app: &App, canvas: &mut Canvas) {
    let y = TITLE_H;
    canvas.fill_rect(0, y, WIN_W, TOOLBAR_H, PANEL);
    // Two pseudo-buttons: Inbox / Compose.
    for (i, (label, view)) in [("Inbox", View::Inbox), ("Compose", View::Compose)]
        .iter()
        .enumerate()
    {
        let bx = 8 + i * 100;
        let active = app.view == *view;
        let bg = if active { ROW_SEL } else { ROW_BG };
        canvas.fill_rounded_rect(
            bx,
            y + 5,
            92,
            TOOLBAR_H - 10,
            ath_tokens::RADIUS_SM as usize,
            bg,
        );
        if active {
            canvas.fill_rect(bx, y + TOOLBAR_H - 5, 92, 2, accent());
        }
        let fg = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        canvas.draw_text_aa(
            (bx + 12) as i32,
            (y + 9) as i32,
            label,
            ath_tokens::TYPE_LABEL,
            fg,
            FontFamily::Sans,
        );
    }
}

#[cfg(not(test))]
fn render_inbox(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TOOLBAR_H;
    let body_h = WIN_H - top - FOOTER_H;

    // Folder rail.
    canvas.fill_rect(0, top, RAIL_W, body_h, RAIL_BG);
    canvas.fill_rect(RAIL_W, top, 1, body_h, STROKE);
    let folders = ["Inbox", "Drafts", "Sent", "Archive"];
    for (i, f) in folders.iter().enumerate() {
        let fy = top + 8 + i * 30;
        let selected = i == 0;
        if selected {
            canvas.fill_rounded_rect(
                6,
                fy,
                RAIL_W - 12,
                26,
                ath_tokens::RADIUS_SM as usize,
                ROW_SEL,
            );
        }
        canvas.draw_text_aa(
            16,
            (fy + 5) as i32,
            f,
            ath_tokens::TYPE_BODY,
            if selected {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            },
            FontFamily::Sans,
        );
    }

    // Message list column.
    let list_x = RAIL_W + 1;
    canvas.fill_rect(list_x + LIST_W, top, 1, body_h, STROKE);
    let msgs = app.model.messages();
    if msgs.is_empty() {
        canvas.draw_text_aa(
            (list_x + 12) as i32,
            (top + 12) as i32,
            "No messages.",
            ath_tokens::TYPE_BODY,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
    } else {
        let row_h = 52usize;
        let max_rows = body_h / row_h;
        for (i, m) in msgs.iter().take(max_rows).enumerate() {
            let ry = top + i * row_h;
            let selected = i == app.sel;
            let bg = if selected { ROW_SEL } else { ROW_BG };
            canvas.fill_rounded_rect(
                list_x + 4,
                ry + 2,
                LIST_W - 8,
                row_h - 4,
                ath_tokens::RADIUS_SM as usize,
                bg,
            );
            // Unread dot.
            if !m.seen {
                canvas.fill_circle(list_x + 12, ry + 14, 3, accent());
            }
            canvas.draw_text_aa(
                (list_x + 22) as i32,
                (ry + 6) as i32,
                &m.from,
                ath_tokens::TYPE_LABEL,
                TEXT_PRIMARY,
                FontFamily::Sans,
            );
            canvas.draw_text_aa(
                (list_x + 22) as i32,
                (ry + 24) as i32,
                &m.subject,
                ath_tokens::TYPE_CAPTION,
                TEXT_SECONDARY,
                FontFamily::Sans,
            );
            canvas.draw_text_aa(
                (list_x + 22) as i32,
                (ry + 38) as i32,
                &m.date,
                ath_tokens::TYPE_CAPTION,
                TEXT_TERTIARY,
                FontFamily::Sans,
            );
        }
    }

    // Reading pane.
    let pane_x = list_x + LIST_W + 1;
    let pane_w = WIN_W - pane_x;
    if let Some(om) = &app.open {
        let mut y = top + 10;
        canvas.draw_text_aa(
            (pane_x + 12) as i32,
            y as i32,
            if om.subject.is_empty() {
                "(no subject)"
            } else {
                &om.subject
            },
            ath_tokens::TYPE_TITLE,
            TEXT_PRIMARY,
            FontFamily::Sans,
        );
        y += 30;
        header_line(canvas, pane_x + 12, &mut y, "From", &om.from);
        header_line(canvas, pane_x + 12, &mut y, "To", &om.to);
        header_line(canvas, pane_x + 12, &mut y, "Date", &om.date);
        canvas.fill_rect(pane_x + 12, y, pane_w - 24, 1, STROKE);
        y += 10;
        // Body — wrapped to the pane width, bounded line count.
        render_body(
            canvas,
            pane_x + 12,
            y,
            pane_w - 24,
            top + body_h - FOOTER_H,
            &om.body,
        );
    } else {
        canvas.draw_text_aa(
            (pane_x + 12) as i32,
            (top + 12) as i32,
            "Select a message to read it.",
            ath_tokens::TYPE_BODY,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
    }
}

#[cfg(not(test))]
fn header_line(canvas: &mut Canvas, x: usize, y: &mut usize, label: &str, value: &str) {
    canvas.draw_text_aa(
        x as i32,
        *y as i32,
        &format!("{}: {}", label, value),
        ath_tokens::TYPE_CAPTION,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );
    *y += 18;
}

/// Word-wrap and draw a body string within `[x, x+w]`, stopping at `y_limit`.
#[cfg(not(test))]
fn render_body(canvas: &mut Canvas, x: usize, mut y: usize, w: usize, y_limit: usize, body: &str) {
    let line_h = ath_tokens::TYPE_BODY.line_height as usize + 4;
    let approx_char_w = 7usize; // monospace-ish budget for wrap decisions
    let max_chars = (w / approx_char_w).max(8);
    for raw_line in body.split('\n') {
        // Hard-wrap each logical line into chunks of max_chars.
        let mut start = 0;
        let bytes: Vec<char> = raw_line.chars().collect();
        if bytes.is_empty() {
            y += line_h;
            if y > y_limit {
                return;
            }
            continue;
        }
        while start < bytes.len() {
            if y > y_limit {
                return;
            }
            let end = (start + max_chars).min(bytes.len());
            let chunk: String = bytes[start..end].iter().collect();
            canvas.draw_text_aa(
                x as i32,
                y as i32,
                &chunk,
                ath_tokens::TYPE_BODY,
                TEXT_PRIMARY,
                FontFamily::Sans,
            );
            y += line_h;
            start = end;
        }
    }
}

#[cfg(not(test))]
fn render_compose(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TOOLBAR_H + 10;
    let x = 16usize;
    let w = WIN_W - 32;
    let d = app.model.draft();

    field(canvas, x, top, w, "To", &d.to);
    field(canvas, x, top + 50, w, "Subject", &d.subject);

    // Body box.
    let body_y = top + 100;
    let body_h = WIN_H - body_y - FOOTER_H - 10;
    canvas.draw_text_aa(
        x as i32,
        body_y as i32,
        "Message",
        ath_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(
        x,
        body_y + 16,
        w,
        body_h,
        ath_tokens::RADIUS_MD as usize,
        ROW_BG,
    );
    render_body(
        canvas,
        x + 8,
        body_y + 24,
        w - 16,
        body_y + 16 + body_h - 8,
        &d.body,
    );
}

#[cfg(not(test))]
fn field(canvas: &mut Canvas, x: usize, y: usize, w: usize, label: &str, value: &str) {
    canvas.draw_text_aa(
        x as i32,
        y as i32,
        label,
        ath_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(x, y + 16, w, 26, ath_tokens::RADIUS_SM as usize, ROW_BG);
    canvas.draw_text_aa(
        (x + 8) as i32,
        (y + 21) as i32,
        value,
        ath_tokens::TYPE_BODY,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );
}

#[cfg(not(test))]
fn render_footer(app: &App, canvas: &mut Canvas) {
    let fy = WIN_H - FOOTER_H;
    canvas.fill_rect(0, fy, WIN_W, FOOTER_H, PANEL);
    let hint = match app.view {
        View::Inbox => "Up/Dn: select  Enter: open  F: fetch  1: inbox  2: compose  Esc: quit",
        View::Compose => "1: inbox   2: compose   Esc: quit   (send is iron-gated on net)",
    };
    canvas.draw_text_aa(
        10,
        fy as i32 + ((FOOTER_H - ath_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        if app.toast.text.is_empty() {
            hint
        } else {
            app.toast.text.as_str()
        },
        ath_tokens::TYPE_CAPTION,
        if app.toast.text.is_empty() {
            TEXT_TERTIARY
        } else {
            accent()
        },
        FontFamily::Sans,
    );
}

// ===========================================================================
// Live entry point.
// ===========================================================================

/// The freestanding userspace entry (called by the `_start` shim in `main.rs`).
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = athkit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        athkit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;

    loop {
        // Mouse: toolbar buttons + message-row selection.
        let mut left_down = false;
        let mut mouse_edge = false;
        loop {
            let ev = athkit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_down {
                mouse_edge = true;
            }
            left_down = now_down;
        }
        if mouse_edge {
            let (cx, cy, _btn) = athkit::sys::cursor_pos();
            let (ox, oy) =
                athkit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            if handle_click(&mut app, lx, ly) {
                render(&app, &mut canvas);
                athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
        }

        let key = athkit::sys::read_key();
        if key == 0 {
            athkit::sys::yield_now();
            continue;
        }

        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        let release = sc & 0x80 != 0;
        let code = sc & 0x7F;
        if release {
            continue;
        }

        let mut changed = true;

        if code == 0x01 {
            // Esc → persist then quit.
            app.persist();
            athkit::sys::exit(0);
        } else if ext && code == 0x48 {
            // Up.
            if app.view == View::Inbox && app.sel > 0 {
                app.sel -= 1;
            } else {
                changed = false;
            }
        } else if ext && code == 0x50 {
            // Down.
            if app.view == View::Inbox && app.sel + 1 < app.model.message_count() {
                app.sel += 1;
            } else {
                changed = false;
            }
        } else if code == 0x1C {
            // Enter → open selected.
            if app.view == View::Inbox {
                app.open_selected();
            } else {
                changed = false;
            }
        } else {
            match code {
                0x02 => {
                    // '1' inbox.
                    app.view = View::Inbox;
                    app.toast.clear();
                }
                0x03 => {
                    // '2' compose.
                    app.view = View::Compose;
                    app.toast.set("Compose: send is iron-gated on networking.");
                }
                0x21 => {
                    // 'f' fetch the inbox over the live socket transport.
                    if app.view == View::Inbox {
                        app.fetch_now();
                    } else {
                        changed = false;
                    }
                }
                _ => changed = false,
            }
        }

        if changed {
            render(&app, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

/// Hit-test a surface-local click. Returns whether anything changed (redraw).
#[cfg(not(test))]
fn handle_click(app: &mut App, lx: i32, ly: i32) -> bool {
    if lx < 0 || ly < 0 {
        return false;
    }
    let lxu = lx as usize;
    let lyu = ly as usize;

    // Toolbar buttons.
    let tb_top = TITLE_H;
    if lyu >= tb_top && lyu < tb_top + TOOLBAR_H {
        if lxu >= 8 && lxu < 100 {
            app.view = View::Inbox;
            app.toast.clear();
            return true;
        } else if lxu >= 108 && lxu < 200 {
            app.view = View::Compose;
            app.toast.set("Compose: send is iron-gated on networking.");
            return true;
        }
        return false;
    }

    // Message list rows.
    if app.view == View::Inbox {
        let top = TITLE_H + TOOLBAR_H;
        let list_x = RAIL_W + 1;
        let row_h = 52usize;
        if lxu >= list_x && lxu < list_x + LIST_W && lyu >= top {
            let i = (lyu - top) / row_h;
            if i < app.model.message_count() {
                app.sel = i;
                app.open_selected();
                return true;
            }
        }
    }
    false
}

// ===========================================================================
// Host KAT — links the LIVE ath_mail / ath_pim / ath_kv engines against a mock
// transport, no kernel, no network. `cargo test -p mail --features host`.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::VecDeque;

    /// The injectable mock transport: replays scripted server bytes and records
    /// every byte the client sent. This is the WHOLE point of the transport seam —
    /// the SMTP/IMAP/POP3 dialogs are proven on the dev box with no socket. (It is
    /// the same shape as `ath_mail`'s own internal `ScriptedTransport`, which is
    /// `pub(crate)`, so the app crate carries its own.)
    struct MockTransport {
        inbox: VecDeque<u8>,
        sent: Vec<u8>,
        reads: usize,
    }

    impl MockTransport {
        fn new(script: &[u8]) -> MockTransport {
            MockTransport {
                inbox: script.iter().copied().collect(),
                sent: Vec::new(),
                reads: 0,
            }
        }
        fn sent_str(&self) -> String {
            String::from_utf8_lossy(&self.sent).into_owned()
        }
        fn sent_contains(&self, needle: &str) -> bool {
            let n = needle.as_bytes();
            !n.is_empty() && self.sent.windows(n.len()).any(|w| w == n)
        }
    }

    impl MailTransport for MockTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), ath_mail::TransportError> {
            self.sent.extend_from_slice(data);
            Ok(())
        }
        fn recv_line(&mut self) -> Result<Vec<u8>, ath_mail::TransportError> {
            self.reads += 1;
            if self.reads > 1_000_000 {
                return Err(ath_mail::TransportError::Io("runaway".into()));
            }
            let mut line = Vec::new();
            loop {
                match self.inbox.pop_front() {
                    Some(b) => {
                        line.push(b);
                        if b == b'\n' {
                            return Ok(line);
                        }
                    }
                    None => return Err(ath_mail::TransportError::Closed),
                }
            }
        }
        fn recv_exact(&mut self, n: usize) -> Result<Vec<u8>, ath_mail::TransportError> {
            self.reads += 1;
            let mut out = Vec::with_capacity(n.min(4096));
            for _ in 0..n {
                match self.inbox.pop_front() {
                    Some(b) => out.push(b),
                    None => return Err(ath_mail::TransportError::Closed),
                }
            }
            Ok(out)
        }
    }

    // A realistic multipart message used by both the IMAP fetch + the reading-pane
    // parse tests. text/plain + text/html alternative; the reader must show the
    // PLAIN part.
    const MULTIPART_BODY: &str = "\
From: Ada Lovelace <ada@analytical.example>\r
To: team@athena.os\r
Subject: Engine notes\r
Date: Mon, 01 Jun 2026 09:30:00 +0000\r
MIME-Version: 1.0\r
Content-Type: multipart/alternative; boundary=\"BOUND42\"\r
\r
--BOUND42\r
Content-Type: text/plain; charset=utf-8\r
\r
The Analytical Engine weaves algebraic patterns.\r
--BOUND42\r
Content-Type: text/html; charset=utf-8\r
\r
<p>The Analytical Engine weaves <b>algebraic</b> patterns.</p>\r
--BOUND42--\r
";

    /// Build an IMAP server script that answers a greeting, LOGIN, SELECT, two
    /// envelope FETCHes, and LOGOUT. The literal `{N}` octet handling is exercised
    /// by the ENVELOPE-bearing FETCH lines (here we use simple inline envelopes).
    fn imap_list_script() -> Vec<u8> {
        let mut s = String::new();
        s.push_str("* OK IMAP4rev1 ready\r\n");
        // LOGIN (tag A0001)
        s.push_str("A0001 OK LOGIN completed\r\n");
        // SELECT (tag A0002) — 2 messages exist
        s.push_str("* 2 EXISTS\r\n");
        s.push_str("* 0 RECENT\r\n");
        s.push_str("* OK [UIDVALIDITY 1] Ok\r\n");
        s.push_str("A0002 OK [READ-WRITE] SELECT completed\r\n");
        // FETCH 1 (tag A0003)
        s.push_str("* 1 FETCH (FLAGS (\\Seen) ENVELOPE (\"Mon, 01 Jun 2026 09:30:00 +0000\" \"Engine notes\" ((\"Ada Lovelace\" NIL \"ada\" \"analytical.example\")) NIL NIL ((\"team\" NIL \"team\" \"athena.os\")) NIL NIL NIL NIL))\r\n");
        s.push_str("A0003 OK FETCH completed\r\n");
        // FETCH 2 (tag A0004) — unread
        s.push_str("* 2 FETCH (FLAGS () ENVELOPE (\"Tue, 02 Jun 2026 10:00:00 +0000\" \"Re: Engine notes\" ((\"Charles Babbage\" NIL \"charles\" \"difference.example\")) NIL NIL ((\"ada\" NIL \"ada\" \"analytical.example\")) NIL NIL NIL NIL))\r\n");
        s.push_str("A0004 OK FETCH completed\r\n");
        // LOGOUT (tag A0005)
        s.push_str("* BYE logging out\r\n");
        s.push_str("A0005 OK LOGOUT completed\r\n");
        s.into_bytes()
    }

    #[test]
    fn imap_fetch_populates_message_list() {
        let mut model = MailModel::new();
        let mut t = MockTransport::new(&imap_list_script());
        let n = model
            .fetch_imap(&mut t, "INBOX", "ada", "secret")
            .expect("imap fetch");
        assert_eq!(n, 2, "two messages listed");

        // FAIL-able anchors: the client actually issued LOGIN + SELECT + FETCH.
        assert!(t.sent_contains("LOGIN"));
        assert!(t.sent_contains("SELECT"));
        assert!(t.sent_contains("FETCH 1 (FLAGS ENVELOPE)"));

        // Newest (seq 2) first.
        let msgs = model.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].seq, 2);
        assert_eq!(msgs[0].subject, "Re: Engine notes");
        assert_eq!(msgs[0].from, "Charles Babbage <charles@difference.example>");
        assert!(!msgs[0].seen, "seq 2 had no \\Seen flag");

        assert_eq!(msgs[1].seq, 1);
        assert_eq!(msgs[1].subject, "Engine notes");
        assert_eq!(msgs[1].from, "Ada Lovelace <ada@analytical.example>");
        assert!(msgs[1].seen, "seq 1 was \\Seen");
    }

    #[test]
    fn open_multipart_message_parses_headers_and_text_body() {
        // Drive the reading-pane parse directly with a real multipart message.
        let om = MailModel::open_bytes(MULTIPART_BODY.as_bytes());
        assert!(!om.parse_failed);
        // FAIL-able anchors: parsed headers + the chosen text/plain body.
        assert_eq!(om.subject, "Engine notes");
        assert_eq!(om.from, "Ada Lovelace <ada@analytical.example>");
        assert_eq!(om.date, "Mon, 01 Jun 2026 09:30:00 +0000");
        assert_eq!(om.part_count, 2, "alternative has two parts");
        assert_eq!(
            om.body.trim(),
            "The Analytical Engine weaves algebraic patterns.",
            "reader must show the text/plain part, not the HTML"
        );
        assert!(!om.body.contains("<p>"), "must not show the raw HTML part");
    }

    #[test]
    fn pop3_fetch_caches_bodies_and_opens_offline() {
        // POP3 RETRs the full message; the model caches it and can open offline.
        let mut s = String::new();
        s.push_str("+OK POP3 ready\r\n"); // greeting
        s.push_str("+OK user accepted\r\n"); // USER
        s.push_str("+OK pass accepted\r\n"); // PASS
        s.push_str("+OK 1 512\r\n"); // STAT: 1 message
                                     // RETR 1: +OK then the message then a "." terminator line.
        s.push_str("+OK 512 octets\r\n");
        s.push_str(MULTIPART_BODY);
        s.push_str(".\r\n");
        s.push_str("+OK bye\r\n"); // QUIT
        let mut t = MockTransport::new(s.as_bytes());

        let mut model = MailModel::new();
        let n = model
            .fetch_pop3(&mut t, "ada", "secret")
            .expect("pop3 fetch");
        assert_eq!(n, 1);
        assert!(t.sent_contains("USER ada"));
        assert!(t.sent_contains("RETR 1"));

        // The single message is summarized from the parsed headers.
        let msgs = model.messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].subject, "Engine notes");
        assert!(msgs[0].cached, "body cached after RETR");

        // And it opens OFFLINE from the cache (no transport).
        let om = model.open(1).expect("cached body opens");
        assert_eq!(om.subject, "Engine notes");
        assert_eq!(
            om.body.trim(),
            "The Analytical Engine weaves algebraic patterns."
        );
    }

    #[test]
    fn compose_sends_well_formed_rfc822_over_smtp() {
        // SMTP script: greeting, EHLO reply, MAIL FROM ok, RCPT ok, DATA go-ahead,
        // body accepted, QUIT bye.
        let mut s = String::new();
        s.push_str("220 mail.athena.os ESMTP ready\r\n"); // greeting
        s.push_str("250-mail.athena.os\r\n"); // EHLO multiline
        s.push_str("250-PIPELINING\r\n");
        s.push_str("250 SIZE 35882577\r\n");
        s.push_str("250 OK\r\n"); // MAIL FROM
        s.push_str("250 OK\r\n"); // RCPT TO (team@athena.os)
        s.push_str("250 OK\r\n"); // RCPT TO (ops@athena.os)
        s.push_str("354 Start mail input\r\n"); // DATA go-ahead
        s.push_str("250 OK queued\r\n"); // body accepted
        s.push_str("221 Bye\r\n"); // QUIT
        let mut t = MockTransport::new(s.as_bytes());

        let mut model = MailModel::new();
        {
            let d = model.draft_mut();
            d.to = "team@athena.os, ops@athena.os".to_string();
            d.subject = "Shipping Mail".to_string();
            d.body = "The Mail app is live.\nText body.".to_string();
        }
        model
            .send(&mut t, "ada@analytical.example", "", "", "client.athena.os")
            .expect("smtp send");

        // FAIL-able anchors: the client emitted the right envelope commands AND a
        // well-formed RFC822 DATA payload (the actual message bytes, not "Ok").
        let sent = t.sent_str();
        assert!(sent.contains("EHLO client.athena.os"));
        assert!(sent.contains("MAIL FROM:<ada@analytical.example>"));
        assert!(sent.contains("RCPT TO:<team@athena.os>"));
        assert!(sent.contains("RCPT TO:<ops@athena.os>"), "both recipients");
        assert!(sent.contains("DATA"));
        // The serialized RFC822 headers + body landed in the DATA stream.
        assert!(sent.contains("Subject: Shipping Mail"));
        assert!(sent.contains("To: team@athena.os, ops@athena.os"));
        assert!(sent.contains("The Mail app is live."));
        // DATA must be terminated with the lone-dot per RFC 5321.
        assert!(sent.contains("\r\n.\r\n"), "DATA dot-terminated");

        // A successful send clears the draft.
        assert!(model.draft().to.is_empty());
        assert!(model.draft().subject.is_empty());
    }

    #[test]
    fn compose_message_serializes_recipients_and_body() {
        // Assert the built RFC822 message independent of the SMTP dialog.
        let mut model = MailModel::new();
        {
            let d = model.draft_mut();
            d.to = "a@x.example; b@y.example".to_string();
            d.subject = "Hi".to_string();
            d.body = "Line one.\nLine two.".to_string();
        }
        let msg = model.compose_message("me@athena.os");
        assert_eq!(msg.from_addr, "me@athena.os");
        assert_eq!(
            msg.to,
            alloc::vec!["a@x.example".to_string(), "b@y.example".to_string()]
        );
        assert_eq!(msg.recipients().len(), 2);
        let raw = msg.serialize();
        assert!(raw.contains("Subject: Hi\r\n"));
        assert!(raw.contains("Line one."));
    }

    #[test]
    fn hostile_message_bytes_degrade_cleanly() {
        // Truncated mid-header / garbage / empty — must never panic, must flag a
        // parse failure with an explanatory (non-empty) body.
        for raw in [
            &b""[..],
            &b"\xff\xfe\x00garbage not a message"[..],
            &b"From: someone"[..], // header started, never terminated, no body
            &b"Content-Type: multipart/mixed; boundary=\"x\"\r\n\r\n--x\r\n"[..], // dangling boundary
        ] {
            let om = MailModel::open_bytes(raw);
            // Either it parsed into *something* (empty body) or it cleanly flagged a
            // failure — never a panic, and a failure always carries a message.
            if om.parse_failed {
                assert!(!om.body.is_empty(), "parse failure must explain itself");
            }
        }
    }

    #[test]
    fn kv_cache_round_trips_account_and_messages() {
        // Populate via a POP3 fetch, persist to bytes, reload, and assert the list
        // + account survive the LIVE ath_kv snapshot round-trip.
        let mut s = String::new();
        s.push_str("+OK ready\r\n");
        s.push_str("+OK\r\n");
        s.push_str("+OK\r\n");
        s.push_str("+OK 1 512\r\n");
        s.push_str("+OK 512 octets\r\n");
        s.push_str(MULTIPART_BODY);
        s.push_str(".\r\n");
        s.push_str("+OK bye\r\n");
        let mut t = MockTransport::new(s.as_bytes());

        let mut model = MailModel::new();
        model.set_account("imap.athena.os", "ada");
        model.fetch_pop3(&mut t, "ada", "secret").unwrap();
        assert_eq!(model.message_count(), 1);

        let bytes = model.to_cache_bytes();
        assert!(!bytes.is_empty());

        let reloaded = MailModel::from_cache_bytes(&bytes);
        assert_eq!(reloaded.account().host, "imap.athena.os");
        assert_eq!(reloaded.account().user, "ada");
        assert_eq!(reloaded.message_count(), 1, "summary survived the snapshot");
        assert_eq!(reloaded.messages()[0].subject, "Engine notes");
        // And the cached body survived → opens offline after reload.
        let om = reloaded.open(1).expect("body survived snapshot");
        assert_eq!(om.subject, "Engine notes");
    }

    #[test]
    fn vcard_import_powers_compose_autocomplete() {
        const VCF: &str = "\
BEGIN:VCARD\r
VERSION:4.0\r
FN:Grace Hopper\r
EMAIL;TYPE=work:grace@navy.example\r
END:VCARD\r
BEGIN:VCARD\r
VERSION:4.0\r
FN:Alan Turing\r
EMAIL:alan@bletchley.example\r
END:VCARD\r
";
        let mut model = MailModel::new();
        let n = model.import_contacts(VCF);
        assert_eq!(n, 2);
        // Query by name (case-insensitive) and by email substring.
        let hits = model.autocomplete("hopp");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0],
            ("Grace Hopper".to_string(), "grace@navy.example".to_string())
        );
        let by_email = model.autocomplete("bletchley");
        assert_eq!(by_email.len(), 1);
        assert_eq!(by_email[0].0, "Alan Turing");
        // Empty query returns all (bounded).
        assert_eq!(model.autocomplete("").len(), 2);
    }

    #[test]
    fn imap_failure_leaves_existing_list_intact() {
        // First, a good fetch.
        let mut t = MockTransport::new(&imap_list_script());
        let mut model = MailModel::new();
        model.fetch_imap(&mut t, "INBOX", "ada", "secret").unwrap();
        assert_eq!(model.message_count(), 2);

        // Then a fetch against a server that NOs the LOGIN — the prior list stays.
        let mut bad = String::new();
        bad.push_str("* OK ready\r\n");
        bad.push_str("A0001 NO [AUTHENTICATIONFAILED] bad credentials\r\n");
        let mut t2 = MockTransport::new(bad.as_bytes());
        let r = model.fetch_imap(&mut t2, "INBOX", "ada", "wrong");
        assert!(r.is_err(), "login failure surfaces as Err");
        assert_eq!(
            model.message_count(),
            2,
            "failed fetch must not clobber the list"
        );
    }
}
