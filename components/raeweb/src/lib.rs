//! RaeWeb — web rendering engine for RaeenOS.
//!
//! A from-scratch HTML/CSS rendering pipeline: tokenizer → parser → DOM → style
//! resolution → layout → paint.  Designed for `no_std` environments with alloc.
//!
//! > "Native everywhere. No Electron tax. No web wrappers. Native rendering,
//! > native input, native audio — sub-frame latency end to end."
//! > — RaeenOS Concept, §Design Principles #1
//!
//! The web is a *contained app surface*, not an OS primitive: this engine parses and
//! lays out HTML/CSS itself and paints through [`raegfx::Canvas`] (the same crisp-AA
//! path as every RaeUI surface), with the resource loader riding the real
//! `raenet::http1` client. No JS in Phase 1 (the `EventListener` hooks stay inert) —
//! the useful 80% of PWA/static content ships years before a JS engine, which is the
//! whole point of the no-JS-as-system-language stance. See
//! `docs/research/web-engine.md`.
#![no_std]

extern crate alloc;

pub mod backend;
pub mod dom_binding;
pub mod loader;

pub use dom_binding::DomDocument;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

// ═══════════════════════════════════════════════════════════════════════════
//  1.  HTML TOKENIZER + PARSER
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HtmlToken {
    Doctype {
        name: Option<String>,
        public_id: Option<String>,
        system_id: Option<String>,
        force_quirks: bool,
    },
    StartTag {
        name: String,
        self_closing: bool,
        attributes: Vec<Attribute>,
    },
    EndTag {
        name: String,
    },
    Character(char),
    Comment(String),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenizerState {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttrName,
    AttrName,
    AfterAttrName,
    BeforeAttrValue,
    AttrValueDoubleQuoted,
    AttrValueSingleQuoted,
    AttrValueUnquoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    Comment,
    CommentEndDash,
    CommentEnd,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    CdataSection,
}

pub struct HtmlTokenizer {
    input: Vec<char>,
    pos: usize,
    state: TokenizerState,
    current_tag_name: String,
    current_tag_is_end: bool,
    current_tag_self_closing: bool,
    current_attr_name: String,
    current_attr_value: String,
    current_attrs: Vec<Attribute>,
    current_comment: String,
    current_doctype_name: Option<String>,
    current_doctype_public: Option<String>,
    current_doctype_system: Option<String>,
    current_doctype_force_quirks: bool,
    reconsume: bool,
    pending_tokens: Vec<HtmlToken>,
}

impl HtmlTokenizer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            state: TokenizerState::Data,
            current_tag_name: String::new(),
            current_tag_is_end: false,
            current_tag_self_closing: false,
            current_attr_name: String::new(),
            current_attr_value: String::new(),
            current_attrs: Vec::new(),
            current_comment: String::new(),
            current_doctype_name: None,
            current_doctype_public: None,
            current_doctype_system: None,
            current_doctype_force_quirks: false,
            reconsume: false,
            pending_tokens: Vec::new(),
        }
    }

    fn next_char(&mut self) -> Option<char> {
        if self.reconsume {
            self.reconsume = false;
            if self.pos > 0 {
                return Some(self.input[self.pos - 1]);
            }
        }
        if self.pos < self.input.len() {
            let ch = self.input[self.pos];
            self.pos += 1;
            Some(ch)
        } else {
            None
        }
    }

    fn peek_char(&self) -> Option<char> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    fn peek_matches(&self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        if self.pos + chars.len() > self.input.len() {
            return false;
        }
        for (i, &ch) in chars.iter().enumerate() {
            let ic = self.input[self.pos + i];
            if ic.to_ascii_lowercase() != ch.to_ascii_lowercase() {
                return false;
            }
        }
        true
    }

    fn consume_chars(&mut self, n: usize) {
        self.pos += n;
    }

    fn finish_attr(&mut self) {
        if !self.current_attr_name.is_empty() {
            self.current_attrs.push(Attribute {
                name: core::mem::take(&mut self.current_attr_name),
                value: core::mem::take(&mut self.current_attr_value),
            });
        }
        self.current_attr_name.clear();
        self.current_attr_value.clear();
    }

    fn emit_current_tag(&mut self) -> HtmlToken {
        self.finish_attr();
        if self.current_tag_is_end {
            HtmlToken::EndTag {
                name: core::mem::take(&mut self.current_tag_name),
            }
        } else {
            HtmlToken::StartTag {
                name: core::mem::take(&mut self.current_tag_name),
                self_closing: self.current_tag_self_closing,
                attributes: core::mem::take(&mut self.current_attrs),
            }
        }
    }

    fn reset_tag(&mut self) {
        self.current_tag_name.clear();
        self.current_tag_is_end = false;
        self.current_tag_self_closing = false;
        self.current_attrs.clear();
        self.current_attr_name.clear();
        self.current_attr_value.clear();
    }

    pub fn next_token(&mut self) -> HtmlToken {
        if let Some(tok) = self.pending_tokens.pop() {
            return tok;
        }
        loop {
            let ch = self.next_char();
            match self.state {
                TokenizerState::Data => match ch {
                    Some('&') => {
                        if let Some(decoded) = self.consume_entity() {
                            return HtmlToken::Character(decoded);
                        }
                    }
                    Some('<') => self.state = TokenizerState::TagOpen,
                    None => return HtmlToken::Eof,
                    Some(c) => return HtmlToken::Character(c),
                },
                TokenizerState::TagOpen => match ch {
                    Some('!') => self.state = TokenizerState::MarkupDeclarationOpen,
                    Some('/') => self.state = TokenizerState::EndTagOpen,
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.reset_tag();
                        self.current_tag_name.push(c.to_ascii_lowercase());
                        self.state = TokenizerState::TagName;
                    }
                    Some('?') => {
                        self.current_comment.clear();
                        self.state = TokenizerState::BogusComment;
                    }
                    _ => {
                        self.reconsume = true;
                        self.state = TokenizerState::Data;
                        return HtmlToken::Character('<');
                    }
                },
                TokenizerState::EndTagOpen => match ch {
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.reset_tag();
                        self.current_tag_is_end = true;
                        self.current_tag_name.push(c.to_ascii_lowercase());
                        self.state = TokenizerState::TagName;
                    }
                    Some('>') => self.state = TokenizerState::Data,
                    _ => {
                        self.current_comment.clear();
                        self.state = TokenizerState::BogusComment;
                        self.reconsume = true;
                    }
                },
                TokenizerState::TagName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {
                        self.state = TokenizerState::BeforeAttrName;
                    }
                    Some('/') => self.state = TokenizerState::SelfClosingStartTag,
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    None => return HtmlToken::Eof,
                    Some(c) => self.current_tag_name.push(c.to_ascii_lowercase()),
                },
                TokenizerState::BeforeAttrName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {}
                    Some('/') | Some('>') | None => {
                        self.reconsume = true;
                        self.state = TokenizerState::AfterAttrName;
                    }
                    Some('=') => {
                        self.finish_attr();
                        self.current_attr_name.push('=');
                        self.state = TokenizerState::AttrName;
                    }
                    Some(c) => {
                        self.finish_attr();
                        self.current_attr_name.push(c.to_ascii_lowercase());
                        self.state = TokenizerState::AttrName;
                    }
                },
                TokenizerState::AttrName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {
                        self.state = TokenizerState::AfterAttrName;
                    }
                    Some('/') => {
                        self.state = TokenizerState::SelfClosingStartTag;
                    }
                    Some('=') => self.state = TokenizerState::BeforeAttrValue,
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    None => {
                        self.state = TokenizerState::AfterAttrName;
                        self.reconsume = true;
                    }
                    Some(c) => self.current_attr_name.push(c.to_ascii_lowercase()),
                },
                TokenizerState::AfterAttrName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {}
                    Some('/') => self.state = TokenizerState::SelfClosingStartTag,
                    Some('=') => self.state = TokenizerState::BeforeAttrValue,
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    None => return HtmlToken::Eof,
                    Some(c) => {
                        self.finish_attr();
                        self.current_attr_name.push(c.to_ascii_lowercase());
                        self.state = TokenizerState::AttrName;
                    }
                },
                TokenizerState::BeforeAttrValue => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {}
                    Some('"') => self.state = TokenizerState::AttrValueDoubleQuoted,
                    Some('\'') => self.state = TokenizerState::AttrValueSingleQuoted,
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    _ => {
                        self.reconsume = true;
                        self.state = TokenizerState::AttrValueUnquoted;
                    }
                },
                TokenizerState::AttrValueDoubleQuoted => match ch {
                    Some('"') => self.state = TokenizerState::BeforeAttrName,
                    Some('&') => {
                        if let Some(decoded) = self.consume_entity() {
                            self.current_attr_value.push(decoded);
                        }
                    }
                    None => return HtmlToken::Eof,
                    Some(c) => self.current_attr_value.push(c),
                },
                TokenizerState::AttrValueSingleQuoted => match ch {
                    Some('\'') => self.state = TokenizerState::BeforeAttrName,
                    Some('&') => {
                        if let Some(decoded) = self.consume_entity() {
                            self.current_attr_value.push(decoded);
                        }
                    }
                    None => return HtmlToken::Eof,
                    Some(c) => self.current_attr_value.push(c),
                },
                TokenizerState::AttrValueUnquoted => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {
                        self.state = TokenizerState::BeforeAttrName;
                    }
                    Some('&') => {
                        if let Some(decoded) = self.consume_entity() {
                            self.current_attr_value.push(decoded);
                        }
                    }
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    None => return HtmlToken::Eof,
                    Some(c) => self.current_attr_value.push(c),
                },
                TokenizerState::SelfClosingStartTag => match ch {
                    Some('>') => {
                        self.current_tag_self_closing = true;
                        self.state = TokenizerState::Data;
                        return self.emit_current_tag();
                    }
                    None => return HtmlToken::Eof,
                    _ => {
                        self.reconsume = true;
                        self.state = TokenizerState::BeforeAttrName;
                    }
                },
                TokenizerState::BogusComment => match ch {
                    Some('>') | None => {
                        self.state = TokenizerState::Data;
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    Some(c) => self.current_comment.push(c),
                },
                TokenizerState::MarkupDeclarationOpen => {
                    if self.peek_matches("--") {
                        self.consume_chars(2);
                        self.current_comment.clear();
                        self.state = TokenizerState::CommentStart;
                    } else if self.peek_matches("DOCTYPE") {
                        self.consume_chars(7);
                        self.state = TokenizerState::Doctype;
                    } else if self.peek_matches("[CDATA[") {
                        self.consume_chars(7);
                        self.state = TokenizerState::CdataSection;
                    } else {
                        self.current_comment.clear();
                        self.state = TokenizerState::BogusComment;
                    }
                }
                TokenizerState::CommentStart => match ch {
                    Some('-') => self.state = TokenizerState::CommentEndDash,
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    _ => {
                        if let Some(c) = ch {
                            self.current_comment.push(c);
                        }
                        self.state = TokenizerState::Comment;
                    }
                },
                TokenizerState::Comment => match ch {
                    Some('-') => self.state = TokenizerState::CommentEndDash,
                    None => {
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    Some(c) => self.current_comment.push(c),
                },
                TokenizerState::CommentEndDash => match ch {
                    Some('-') => self.state = TokenizerState::CommentEnd,
                    None => {
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    Some(c) => {
                        self.current_comment.push('-');
                        self.current_comment.push(c);
                        self.state = TokenizerState::Comment;
                    }
                },
                TokenizerState::CommentEnd => match ch {
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    Some('-') => self.current_comment.push('-'),
                    None => {
                        let c = core::mem::take(&mut self.current_comment);
                        return HtmlToken::Comment(c);
                    }
                    Some(c) => {
                        self.current_comment.push('-');
                        self.current_comment.push('-');
                        self.current_comment.push(c);
                        self.state = TokenizerState::Comment;
                    }
                },
                TokenizerState::Doctype => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {
                        self.state = TokenizerState::BeforeDoctypeName;
                    }
                    Some('>') => {
                        self.reconsume = true;
                        self.state = TokenizerState::BeforeDoctypeName;
                    }
                    None => {
                        self.current_doctype_force_quirks = true;
                        return self.emit_doctype();
                    }
                    _ => {
                        self.reconsume = true;
                        self.state = TokenizerState::BeforeDoctypeName;
                    }
                },
                TokenizerState::BeforeDoctypeName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {}
                    Some('>') => {
                        self.current_doctype_force_quirks = true;
                        self.state = TokenizerState::Data;
                        return self.emit_doctype();
                    }
                    None => {
                        self.current_doctype_force_quirks = true;
                        return self.emit_doctype();
                    }
                    Some(c) => {
                        self.current_doctype_name = Some(String::new());
                        if let Some(ref mut n) = self.current_doctype_name {
                            n.push(c.to_ascii_lowercase());
                        }
                        self.state = TokenizerState::DoctypeName;
                    }
                },
                TokenizerState::DoctypeName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {
                        self.state = TokenizerState::AfterDoctypeName;
                    }
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_doctype();
                    }
                    None => {
                        self.current_doctype_force_quirks = true;
                        return self.emit_doctype();
                    }
                    Some(c) => {
                        if let Some(ref mut n) = self.current_doctype_name {
                            n.push(c.to_ascii_lowercase());
                        }
                    }
                },
                TokenizerState::AfterDoctypeName => match ch {
                    Some('\t') | Some('\n') | Some('\x0C') | Some(' ') => {}
                    Some('>') => {
                        self.state = TokenizerState::Data;
                        return self.emit_doctype();
                    }
                    None => {
                        self.current_doctype_force_quirks = true;
                        return self.emit_doctype();
                    }
                    _ => {
                        self.state = TokenizerState::AfterDoctypeName;
                    }
                },
                TokenizerState::CdataSection => match ch {
                    None => return HtmlToken::Eof,
                    Some(']') => {
                        if self.peek_matches("]>") {
                            self.consume_chars(2);
                            self.state = TokenizerState::Data;
                        } else {
                            return HtmlToken::Character(']');
                        }
                    }
                    Some(c) => return HtmlToken::Character(c),
                },
            }
        }
    }

    fn emit_doctype(&mut self) -> HtmlToken {
        let tok = HtmlToken::Doctype {
            name: self.current_doctype_name.take(),
            public_id: self.current_doctype_public.take(),
            system_id: self.current_doctype_system.take(),
            force_quirks: self.current_doctype_force_quirks,
        };
        self.current_doctype_force_quirks = false;
        self.state = TokenizerState::Data;
        tok
    }

    fn consume_entity(&mut self) -> Option<char> {
        if self.pos >= self.input.len() {
            return Some('&');
        }
        let start = self.pos;
        if self.peek_char() == Some('#') {
            self.pos += 1;
            let hex = self.peek_char() == Some('x') || self.peek_char() == Some('X');
            if hex {
                self.pos += 1;
            }
            let num_start = self.pos;
            while self.pos < self.input.len() {
                let c = self.input[self.pos];
                if hex && c.is_ascii_hexdigit() {
                    self.pos += 1;
                } else if !hex && c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if self.pos == num_start {
                self.pos = start;
                return Some('&');
            }
            let slice: String = self.input[num_start..self.pos].iter().collect();
            if self.pos < self.input.len() && self.input[self.pos] == ';' {
                self.pos += 1;
            }
            let code = if hex {
                u32::from_str_radix(&slice, 16).unwrap_or(0xFFFD)
            } else {
                parse_u32_decimal(&slice).unwrap_or(0xFFFD)
            };
            return char::from_u32(code).or(Some('\u{FFFD}'));
        }

        let mut name = String::new();
        while self.pos < self.input.len() && self.input[self.pos].is_ascii_alphanumeric() {
            name.push(self.input[self.pos]);
            self.pos += 1;
            if self.pos < self.input.len() && self.input[self.pos] == ';' {
                self.pos += 1;
                break;
            }
        }
        match name.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{00A0}'),
            "copy" => Some('\u{00A9}'),
            "reg" => Some('\u{00AE}'),
            "trade" => Some('\u{2122}'),
            "hellip" => Some('\u{2026}'),
            "mdash" => Some('\u{2014}'),
            "ndash" => Some('\u{2013}'),
            "laquo" => Some('\u{00AB}'),
            "raquo" => Some('\u{00BB}'),
            "bull" => Some('\u{2022}'),
            "middot" => Some('\u{00B7}'),
            // Smart quotes / typography — pervasive on real prose pages.
            "lsquo" => Some('\u{2018}'),
            "rsquo" => Some('\u{2019}'),
            "ldquo" => Some('\u{201C}'),
            "rdquo" => Some('\u{201D}'),
            "sbquo" => Some('\u{201A}'),
            "bdquo" => Some('\u{201E}'),
            "prime" => Some('\u{2032}'),
            "Prime" => Some('\u{2033}'),
            "dagger" => Some('\u{2020}'),
            "Dagger" => Some('\u{2021}'),
            "permil" => Some('\u{2030}'),
            // Spaces / dashes / joiners.
            "ensp" => Some('\u{2002}'),
            "emsp" => Some('\u{2003}'),
            "thinsp" => Some('\u{2009}'),
            "shy" => Some('\u{00AD}'),
            "zwnj" => Some('\u{200C}'),
            "zwj" => Some('\u{200D}'),
            "minus" => Some('\u{2212}'),
            // Currency.
            "cent" => Some('\u{00A2}'),
            "pound" => Some('\u{00A3}'),
            "curren" => Some('\u{00A4}'),
            "yen" => Some('\u{00A5}'),
            "euro" => Some('\u{20AC}'),
            // Common symbols / math.
            "sect" => Some('\u{00A7}'),
            "para" => Some('\u{00B6}'),
            "deg" => Some('\u{00B0}'),
            "plusmn" => Some('\u{00B1}'),
            "times" => Some('\u{00D7}'),
            "divide" => Some('\u{00F7}'),
            "micro" => Some('\u{00B5}'),
            "sup1" => Some('\u{00B9}'),
            "sup2" => Some('\u{00B2}'),
            "sup3" => Some('\u{00B3}'),
            "frac12" => Some('\u{00BD}'),
            "frac14" => Some('\u{00BC}'),
            "frac34" => Some('\u{00BE}'),
            "iexcl" => Some('\u{00A1}'),
            "iquest" => Some('\u{00BF}'),
            "brvbar" => Some('\u{00A6}'),
            "uml" => Some('\u{00A8}'),
            "ordf" => Some('\u{00AA}'),
            "ordm" => Some('\u{00BA}'),
            "not" => Some('\u{00AC}'),
            "macr" => Some('\u{00AF}'),
            "acute" => Some('\u{00B4}'),
            "cedil" => Some('\u{00B8}'),
            "larr" => Some('\u{2190}'),
            "uarr" => Some('\u{2191}'),
            "rarr" => Some('\u{2192}'),
            "darr" => Some('\u{2193}'),
            "harr" => Some('\u{2194}'),
            "spades" => Some('\u{2660}'),
            "clubs" => Some('\u{2663}'),
            "hearts" => Some('\u{2665}'),
            "diams" => Some('\u{2666}'),
            "loz" => Some('\u{25CA}'),
            // Latin-1 accented letters (uppercase) — names, FR/DE/ES text.
            "Agrave" => Some('\u{00C0}'),
            "Aacute" => Some('\u{00C1}'),
            "Acirc" => Some('\u{00C2}'),
            "Atilde" => Some('\u{00C3}'),
            "Auml" => Some('\u{00C4}'),
            "Aring" => Some('\u{00C5}'),
            "AElig" => Some('\u{00C6}'),
            "Ccedil" => Some('\u{00C7}'),
            "Egrave" => Some('\u{00C8}'),
            "Eacute" => Some('\u{00C9}'),
            "Ecirc" => Some('\u{00CA}'),
            "Euml" => Some('\u{00CB}'),
            "Igrave" => Some('\u{00CC}'),
            "Iacute" => Some('\u{00CD}'),
            "Icirc" => Some('\u{00CE}'),
            "Iuml" => Some('\u{00CF}'),
            "ETH" => Some('\u{00D0}'),
            "Ntilde" => Some('\u{00D1}'),
            "Ograve" => Some('\u{00D2}'),
            "Oacute" => Some('\u{00D3}'),
            "Ocirc" => Some('\u{00D4}'),
            "Otilde" => Some('\u{00D5}'),
            "Ouml" => Some('\u{00D6}'),
            "Oslash" => Some('\u{00D8}'),
            "Ugrave" => Some('\u{00D9}'),
            "Uacute" => Some('\u{00DA}'),
            "Ucirc" => Some('\u{00DB}'),
            "Uuml" => Some('\u{00DC}'),
            "Yacute" => Some('\u{00DD}'),
            "THORN" => Some('\u{00DE}'),
            "szlig" => Some('\u{00DF}'),
            // Latin-1 accented letters (lowercase).
            "agrave" => Some('\u{00E0}'),
            "aacute" => Some('\u{00E1}'),
            "acirc" => Some('\u{00E2}'),
            "atilde" => Some('\u{00E3}'),
            "auml" => Some('\u{00E4}'),
            "aring" => Some('\u{00E5}'),
            "aelig" => Some('\u{00E6}'),
            "ccedil" => Some('\u{00E7}'),
            "egrave" => Some('\u{00E8}'),
            "eacute" => Some('\u{00E9}'),
            "ecirc" => Some('\u{00EA}'),
            "euml" => Some('\u{00EB}'),
            "igrave" => Some('\u{00EC}'),
            "iacute" => Some('\u{00ED}'),
            "icirc" => Some('\u{00EE}'),
            "iuml" => Some('\u{00EF}'),
            "eth" => Some('\u{00F0}'),
            "ntilde" => Some('\u{00F1}'),
            "ograve" => Some('\u{00F2}'),
            "oacute" => Some('\u{00F3}'),
            "ocirc" => Some('\u{00F4}'),
            "otilde" => Some('\u{00F5}'),
            "ouml" => Some('\u{00F6}'),
            "oslash" => Some('\u{00F8}'),
            "ugrave" => Some('\u{00F9}'),
            "uacute" => Some('\u{00FA}'),
            "ucirc" => Some('\u{00FB}'),
            "uuml" => Some('\u{00FC}'),
            "yacute" => Some('\u{00FD}'),
            "thorn" => Some('\u{00FE}'),
            "yuml" => Some('\u{00FF}'),
            _ => {
                self.pos = start;
                Some('&')
            }
        }
    }

    pub fn tokenize_all(&mut self) -> Vec<HtmlToken> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            if tok == HtmlToken::Eof {
                tokens.push(tok);
                break;
            }
            tokens.push(tok);
        }
        tokens
    }
}

fn parse_u32_decimal(s: &str) -> Option<u32> {
    let mut result: u32 = 0;
    for ch in s.chars() {
        let d = ch.to_digit(10)?;
        result = result.checked_mul(10)?.checked_add(d)?;
    }
    Some(result)
}

// ── HTML Tree Builder ─────────────────────────────────────────────────────

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

fn is_void_element(tag: &str) -> bool {
    VOID_ELEMENTS.contains(&tag)
}

/// Block-level start tags that imply a `</p>` end tag (HTML "optional end tags").
const P_CLOSERS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "details",
    "div",
    "dl",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "main",
    "menu",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "ul",
];

/// Should opening `new_tag` implicitly close a currently-open `open` element?
/// Covers the common optional-end-tag cases (lists, paragraphs, definition lists,
/// options, table rows/cells) so unclosed markup nests as siblings, not children.
fn implies_close(new_tag: &str, open: &str) -> bool {
    match open {
        "li" => new_tag == "li",
        "dd" | "dt" => new_tag == "dd" || new_tag == "dt",
        "option" => new_tag == "option" || new_tag == "optgroup",
        "optgroup" => new_tag == "optgroup",
        "p" => P_CLOSERS.contains(&new_tag),
        "td" | "th" => matches!(new_tag, "td" | "th" | "tr" | "tbody" | "thead" | "tfoot"),
        "tr" => matches!(new_tag, "tr" | "tbody" | "thead" | "tfoot"),
        "thead" | "tbody" | "tfoot" => matches!(new_tag, "tbody" | "thead" | "tfoot"),
        _ => false,
    }
}

pub fn parse_html(input: &str) -> DomNode {
    let mut tokenizer = HtmlTokenizer::new(input);
    struct TreeSink {
        nodes: Vec<DomNode>,
        open_stack: Vec<usize>,
    }

    impl TreeSink {
        fn new() -> Self {
            let doc = DomNode::new_document();
            Self {
                nodes: vec![doc],
                open_stack: vec![0],
            }
        }

        fn current(&self) -> usize {
            *self.open_stack.last().unwrap_or(&0)
        }

        fn append_node(&mut self, node: DomNode) -> usize {
            let idx = self.nodes.len();
            self.nodes.push(node);
            let parent = self.current();
            self.nodes[parent].child_indices.push(idx);
            idx
        }

        fn into_dom(self) -> DomNode {
            fn build(idx: usize, nodes: &[DomNode]) -> DomNode {
                let src = &nodes[idx];
                let mut node = DomNode {
                    node_type: src.node_type.clone(),
                    children: Vec::new(),
                    child_indices: Vec::new(),
                    _parent_index: None,
                    attributes: src.attributes.clone(),
                    event_listeners: Vec::new(),
                };
                for &ci in &src.child_indices {
                    let child = build(ci, nodes);
                    node.children.push(child);
                }
                node
            }
            build(0, &self.nodes)
        }
    }

    let mut sink = TreeSink::new();
    let mut text_buf = String::new();
    loop {
        let token = tokenizer.next_token();
        // Coalesce consecutive character tokens into ONE text node. Previously each
        // character became its own text node -> per-character layout boxes (a perf
        // smell) and broke word-boundary-aware processing (e.g. text-transform).
        if let HtmlToken::Character(c) = &token {
            text_buf.push(*c);
            continue;
        }
        if !text_buf.is_empty() {
            sink.append_node(DomNode::new_text(&text_buf));
            text_buf.clear();
        }
        match token {
            HtmlToken::Doctype { name, .. } => {
                let dt = DomNode::new_doctype(name.unwrap_or_default());
                sink.append_node(dt);
            }
            HtmlToken::StartTag {
                name,
                self_closing,
                attributes,
            } => {
                // Implicit closing: pop any open elements whose end tag this start
                // tag implies (e.g. `<li>` closes an open `<li>`; a block start tag
                // closes an open `<p>`).
                while sink.open_stack.len() > 1 {
                    let top = *sink.open_stack.last().unwrap();
                    let open_name = match &sink.nodes[top].node_type {
                        NodeType::Element { tag_name, .. } => tag_name.clone(),
                        _ => break,
                    };
                    if implies_close(&name, &open_name) {
                        sink.open_stack.pop();
                    } else {
                        break;
                    }
                }
                let mut el = DomNode::new_element(&name);
                for attr in &attributes {
                    el.attributes.push(Attribute {
                        name: attr.name.clone(),
                        value: attr.value.clone(),
                    });
                }
                let idx = sink.append_node(el);
                if !self_closing && !is_void_element(&name) {
                    sink.open_stack.push(idx);
                }
            }
            HtmlToken::EndTag { name } => {
                let mut i = sink.open_stack.len();
                while i > 1 {
                    i -= 1;
                    let idx = sink.open_stack[i];
                    if let NodeType::Element { ref tag_name, .. } = sink.nodes[idx].node_type {
                        if *tag_name == name {
                            sink.open_stack.truncate(i);
                            break;
                        }
                    }
                }
            }
            HtmlToken::Character(_) => {} // accumulated into text_buf above
            HtmlToken::Comment(data) => {
                let cm = DomNode::new_comment(&data);
                sink.append_node(cm);
            }
            HtmlToken::Eof => break,
        }
    }
    sink.into_dom()
}

// ═══════════════════════════════════════════════════════════════════════════
//  2.  DOM TREE
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum NodeType {
    Document,
    DocumentType {
        name: String,
    },
    Element {
        tag_name: String,
        namespace: String,
        id: Option<String>,
        class_list: Vec<String>,
    },
    Text(String),
    Comment(String),
    DocumentFragment,
}

pub type EventCallback = fn(&DomEvent);

#[derive(Clone)]
pub struct EventListener {
    pub event_type: String,
    pub callback_id: u64,
}

impl fmt::Debug for EventListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventListener")
            .field("event_type", &self.event_type)
            .field("callback_id", &self.callback_id)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct DomEvent {
    pub event_type: String,
    pub target_tag: String,
    pub bubbles: bool,
    pub cancelable: bool,
    pub default_prevented: bool,
    pub x: f32,
    pub y: f32,
}

impl DomEvent {
    pub fn new(event_type: &str) -> Self {
        Self {
            event_type: String::from(event_type),
            target_tag: String::new(),
            bubbles: true,
            cancelable: true,
            default_prevented: false,
            x: 0.0,
            y: 0.0,
        }
    }

    pub fn prevent_default(&mut self) {
        if self.cancelable {
            self.default_prevented = true;
        }
    }
}

#[derive(Debug, Clone)]
pub struct DomNode {
    pub node_type: NodeType,
    pub children: Vec<DomNode>,
    pub attributes: Vec<Attribute>,
    pub event_listeners: Vec<EventListener>,
    child_indices: Vec<usize>,
    _parent_index: Option<usize>,
}

impl DomNode {
    pub fn new_document() -> Self {
        Self {
            node_type: NodeType::Document,
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn new_document_fragment() -> Self {
        Self {
            node_type: NodeType::DocumentFragment,
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn new_doctype(name: String) -> Self {
        Self {
            node_type: NodeType::DocumentType { name },
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn new_element(tag: &str) -> Self {
        Self {
            node_type: NodeType::Element {
                tag_name: tag.to_string(),
                namespace: String::from("http://www.w3.org/1999/xhtml"),
                id: None,
                class_list: Vec::new(),
            },
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn new_text(data: &str) -> Self {
        Self {
            node_type: NodeType::Text(data.to_string()),
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn new_comment(data: &str) -> Self {
        Self {
            node_type: NodeType::Comment(data.to_string()),
            children: Vec::new(),
            attributes: Vec::new(),
            event_listeners: Vec::new(),
            child_indices: Vec::new(),
            _parent_index: None,
        }
    }

    pub fn tag_name(&self) -> Option<&str> {
        match &self.node_type {
            NodeType::Element { tag_name, .. } => Some(tag_name.as_str()),
            _ => None,
        }
    }

    pub fn element_id(&self) -> Option<&str> {
        self.get_attribute("id")
    }

    pub fn class_list(&self) -> Vec<&str> {
        if let Some(cls) = self.get_attribute("class") {
            cls.split_whitespace().collect()
        } else {
            Vec::new()
        }
    }

    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.value.as_str())
    }

    pub fn set_attribute(&mut self, name: &str, value: &str) {
        if let Some(attr) = self.attributes.iter_mut().find(|a| a.name == name) {
            attr.value = value.to_string();
        } else {
            self.attributes.push(Attribute {
                name: name.to_string(),
                value: value.to_string(),
            });
        }
        if name == "id" {
            if let NodeType::Element { ref mut id, .. } = self.node_type {
                *id = Some(value.to_string());
            }
        }
        if name == "class" {
            if let NodeType::Element {
                ref mut class_list, ..
            } = self.node_type
            {
                *class_list = value.split_whitespace().map(|s| s.to_string()).collect();
            }
        }
    }

    pub fn remove_attribute(&mut self, name: &str) {
        self.attributes.retain(|a| a.name != name);
    }

    pub fn append_child(&mut self, child: DomNode) {
        self.children.push(child);
    }

    pub fn insert_before(&mut self, new_child: DomNode, reference_index: usize) {
        if reference_index <= self.children.len() {
            self.children.insert(reference_index, new_child);
        } else {
            self.children.push(new_child);
        }
    }

    pub fn remove_child(&mut self, index: usize) -> Option<DomNode> {
        if index < self.children.len() {
            Some(self.children.remove(index))
        } else {
            None
        }
    }

    pub fn replace_child(&mut self, new_child: DomNode, index: usize) -> Option<DomNode> {
        if index < self.children.len() {
            let old = core::mem::replace(&mut self.children[index], new_child);
            Some(old)
        } else {
            None
        }
    }

    pub fn clone_node(&self, deep: bool) -> DomNode {
        if deep {
            self.clone()
        } else {
            DomNode {
                node_type: self.node_type.clone(),
                children: Vec::new(),
                attributes: self.attributes.clone(),
                event_listeners: Vec::new(),
                child_indices: Vec::new(),
                _parent_index: None,
            }
        }
    }

    pub fn text_content(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    /// Set this node's `textContent` (the DOM setter): replace ALL children with a single
    /// text node carrying `text`. This is the live mutation a script performs via
    /// `el.textContent = '...'`; the page must be re-laid-out afterward for it to show (the
    /// caller flips a dirty flag — see [`crate::DomDocument`]). A text/comment node's own
    /// data is replaced directly. Never panics.
    pub fn set_text_content(&mut self, text: &str) {
        match &mut self.node_type {
            NodeType::Text(data) | NodeType::Comment(data) => {
                *data = text.to_string();
            }
            _ => {
                self.children.clear();
                if !text.is_empty() {
                    self.children.push(DomNode::new_text(text));
                }
            }
        }
    }

    /// Depth-first search for the first element with `id`, returning a MUTABLE reference so a
    /// caller (the JS DOM binding) can mutate it in place. Mirrors [`get_element_by_id`] but
    /// for writes. `None` if no element carries the id.
    pub fn get_element_by_id_mut(&mut self, id: &str) -> Option<&mut DomNode> {
        if self.element_id() == Some(id) {
            return Some(self);
        }
        for child in &mut self.children {
            if let Some(found) = child.get_element_by_id_mut(id) {
                return Some(found);
            }
        }
        None
    }

    fn collect_text(&self, out: &mut String) {
        match &self.node_type {
            NodeType::Text(data) => out.push_str(data),
            _ => {
                for child in &self.children {
                    child.collect_text(out);
                }
            }
        }
    }

    pub fn inner_html(&self) -> String {
        let mut out = String::new();
        for child in &self.children {
            child.serialize_html(&mut out);
        }
        out
    }

    fn serialize_html(&self, out: &mut String) {
        match &self.node_type {
            NodeType::Element { tag_name, .. } => {
                out.push('<');
                out.push_str(tag_name);
                for attr in &self.attributes {
                    out.push(' ');
                    out.push_str(&attr.name);
                    out.push_str("=\"");
                    for c in attr.value.chars() {
                        match c {
                            '&' => out.push_str("&amp;"),
                            '"' => out.push_str("&quot;"),
                            '<' => out.push_str("&lt;"),
                            '>' => out.push_str("&gt;"),
                            _ => out.push(c),
                        }
                    }
                    out.push('"');
                }
                if is_void_element(tag_name) {
                    out.push_str(" />");
                    return;
                }
                out.push('>');
                for child in &self.children {
                    child.serialize_html(out);
                }
                out.push_str("</");
                out.push_str(tag_name);
                out.push('>');
            }
            NodeType::Text(data) => {
                for c in data.chars() {
                    match c {
                        '&' => out.push_str("&amp;"),
                        '<' => out.push_str("&lt;"),
                        '>' => out.push_str("&gt;"),
                        _ => out.push(c),
                    }
                }
            }
            NodeType::Comment(data) => {
                out.push_str("<!--");
                out.push_str(data);
                out.push_str("-->");
            }
            NodeType::DocumentType { name } => {
                out.push_str("<!DOCTYPE ");
                out.push_str(name);
                out.push('>');
            }
            _ => {
                for child in &self.children {
                    child.serialize_html(out);
                }
            }
        }
    }

    pub fn get_element_by_id(&self, id: &str) -> Option<&DomNode> {
        if self.element_id() == Some(id) {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.get_element_by_id(id) {
                return Some(found);
            }
        }
        None
    }

    pub fn get_elements_by_tag_name(&self, tag: &str) -> Vec<&DomNode> {
        let mut results = Vec::new();
        self.collect_by_tag(tag, &mut results);
        results
    }

    fn collect_by_tag<'a>(&'a self, tag: &str, out: &mut Vec<&'a DomNode>) {
        if let Some(my_tag) = self.tag_name() {
            if my_tag == tag || tag == "*" {
                out.push(self);
            }
        }
        for child in &self.children {
            child.collect_by_tag(tag, out);
        }
    }

    pub fn get_elements_by_class_name(&self, class: &str) -> Vec<&DomNode> {
        let mut results = Vec::new();
        self.collect_by_class(class, &mut results);
        results
    }

    fn collect_by_class<'a>(&'a self, class: &str, out: &mut Vec<&'a DomNode>) {
        if self.class_list().contains(&class) {
            out.push(self);
        }
        for child in &self.children {
            child.collect_by_class(class, out);
        }
    }

    pub fn query_selector(&self, selector_str: &str) -> Option<&DomNode> {
        let selector = parse_single_selector(selector_str)?;
        self.find_matching(&selector, &[], &[])
    }

    pub fn query_selector_all(&self, selector_str: &str) -> Vec<&DomNode> {
        let mut results = Vec::new();
        if let Some(selector) = parse_single_selector(selector_str) {
            self.collect_matching(&selector, &[], &[], &mut results);
        }
        results
    }

    // `ancestors` (root→parent) and `prev_siblings` (preceding element siblings) are
    // threaded so combinator selectors (`div p`, `ul > li`, `h1 + p`) resolve
    // correctly — see [`selector_matches_ctx`]. The context node starts empty
    // (correct for a document-root query).
    fn find_matching<'a>(
        &'a self,
        sel: &CssSelector,
        ancestors: &[&'a DomNode],
        prev_siblings: &[&'a DomNode],
    ) -> Option<&'a DomNode> {
        if selector_matches_ctx(self, sel, ancestors, prev_siblings) {
            return Some(self);
        }
        let mut child_anc: Vec<&DomNode> = ancestors.to_vec();
        child_anc.push(self);
        let mut prev_sibs: Vec<&DomNode> = Vec::new();
        for child in &self.children {
            if let Some(found) = child.find_matching(sel, &child_anc, &prev_sibs) {
                return Some(found);
            }
            if child.is_element() {
                prev_sibs.push(child);
            }
        }
        None
    }

    fn collect_matching<'a>(
        &'a self,
        sel: &CssSelector,
        ancestors: &[&'a DomNode],
        prev_siblings: &[&'a DomNode],
        out: &mut Vec<&'a DomNode>,
    ) {
        if selector_matches_ctx(self, sel, ancestors, prev_siblings) {
            out.push(self);
        }
        let mut child_anc: Vec<&DomNode> = ancestors.to_vec();
        child_anc.push(self);
        let mut prev_sibs: Vec<&DomNode> = Vec::new();
        for child in &self.children {
            child.collect_matching(sel, &child_anc, &prev_sibs, out);
            if child.is_element() {
                prev_sibs.push(child);
            }
        }
    }

    pub fn add_event_listener(&mut self, event_type: &str, callback_id: u64) {
        self.event_listeners.push(EventListener {
            event_type: event_type.to_string(),
            callback_id,
        });
    }

    pub fn remove_event_listener(&mut self, event_type: &str, callback_id: u64) {
        self.event_listeners
            .retain(|l| l.event_type != event_type || l.callback_id != callback_id);
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    pub fn first_child(&self) -> Option<&DomNode> {
        self.children.first()
    }

    pub fn last_child(&self) -> Option<&DomNode> {
        self.children.last()
    }

    pub fn is_element(&self) -> bool {
        matches!(self.node_type, NodeType::Element { .. })
    }

    pub fn child_element_count(&self) -> usize {
        self.children.iter().filter(|c| c.is_element()).count()
    }
}

fn matches_selector(node: &DomNode, sel: &CssSelector) -> bool {
    match sel {
        CssSelector::Universal => node.is_element(),
        CssSelector::Type(tag) => node.tag_name() == Some(tag.as_str()),
        CssSelector::Class(cls) => node.class_list().contains(&cls.as_str()),
        CssSelector::Id(id) => node.element_id() == Some(id.as_str()),
        CssSelector::Attribute { name, op, value } => {
            if let Some(attr_val) = node.get_attribute(name) {
                match op {
                    AttrOp::Exists => true,
                    AttrOp::Equals => value.as_deref() == Some(attr_val),
                    AttrOp::Contains => value.as_deref().map_or(false, |v| attr_val.contains(v)),
                    AttrOp::StartsWith => {
                        value.as_deref().map_or(false, |v| attr_val.starts_with(v))
                    }
                    AttrOp::EndsWith => value.as_deref().map_or(false, |v| attr_val.ends_with(v)),
                }
            } else {
                false
            }
        }
        CssSelector::PseudoClass(pc) => match pc {
            PseudoClass::Hover | PseudoClass::Focus | PseudoClass::Active => false,
            PseudoClass::FirstChild => true,
            PseudoClass::LastChild => true,
            PseudoClass::NthChild(_) => true,
            PseudoClass::Not(inner) => !matches_selector(node, inner),
        },
        CssSelector::Compound(parts) => parts.iter().all(|p| matches_selector(node, p)),
        CssSelector::Combinator { right, .. } => matches_selector(node, right),
    }
}

/// Selector matching WITH ancestor context, so descendant (` `) and child (`>`)
/// combinators actually constrain the match instead of degrading to "the rightmost
/// compound matches" — the old behavior applied every `.nav a` rule to *every*
/// `<a>` on the page. `ancestors` is the chain root→parent (parent last).
///
/// Sibling combinators (`+`, `~`) need preceding-sibling context the styler does
/// not thread, so a chain containing one falls back to the lenient subject-only
/// match (unchanged from before — never a regression). Used by both the render
/// styler (`build_styled_tree`) and the DOM query path (`querySelectorAll`).
fn selector_matches_ctx(
    node: &DomNode,
    sel: &CssSelector,
    ancestors: &[&DomNode],
    prev_siblings: &[&DomNode],
) -> bool {
    let CssSelector::Combinator { .. } = sel else {
        return matches_selector(node, sel);
    };
    let mut compounds: Vec<&CssSelector> = Vec::new();
    let mut combs: Vec<&CssCombinator> = Vec::new();
    flatten_combinator(sel, &mut compounds, &mut combs);
    if compounds.is_empty() {
        return false;
    }
    match_subchain(
        node,
        &compounds,
        &combs,
        compounds.len() - 1,
        ancestors,
        prev_siblings,
    )
}

/// Does `elem` satisfy the selector sub-chain `compounds[0..=end]` (with `combs[i]`
/// connecting `compounds[i]`->`compounds[i+1]`)? Walks right-to-left: descendant =
/// some ancestor matches the remaining left sub-chain; child = the immediate parent;
/// adjacent/general sibling = a preceding sibling (nearest / any). Siblings share
/// `elem`'s ancestors, so those carry through; but stepping UP to an ancestor drops
/// that ancestor's siblings (passed empty), so a sibling combinator to the LEFT of an
/// ancestor step (`h1 + div p`) safely under-matches rather than over-applies.
fn match_subchain(
    elem: &DomNode,
    compounds: &[&CssSelector],
    combs: &[&CssCombinator],
    end: usize,
    ancestors: &[&DomNode],
    prev: &[&DomNode],
) -> bool {
    if !matches_selector(elem, compounds[end]) {
        return false;
    }
    if end == 0 {
        return true;
    }
    match combs[end - 1] {
        CssCombinator::Descendant => (0..ancestors.len()).rev().any(|k| {
            match_subchain(
                ancestors[k],
                compounds,
                combs,
                end - 1,
                &ancestors[..k],
                &[],
            )
        }),
        CssCombinator::Child => {
            let k = ancestors.len();
            k > 0
                && match_subchain(
                    ancestors[k - 1],
                    compounds,
                    combs,
                    end - 1,
                    &ancestors[..k - 1],
                    &[],
                )
        }
        CssCombinator::AdjacentSibling => match prev.split_last() {
            Some((sib, rest)) => match_subchain(sib, compounds, combs, end - 1, ancestors, rest),
            None => false,
        },
        CssCombinator::GeneralSibling => (0..prev.len())
            .rev()
            .any(|j| match_subchain(prev[j], compounds, combs, end - 1, ancestors, &prev[..j])),
    }
}

fn flatten_combinator<'a>(
    sel: &'a CssSelector,
    compounds: &mut Vec<&'a CssSelector>,
    combs: &mut Vec<&'a CssCombinator>,
) {
    match sel {
        CssSelector::Combinator {
            left,
            combinator,
            right,
        } => {
            compounds.push(left);
            combs.push(combinator);
            flatten_combinator(right, compounds, combs);
        }
        other => compounds.push(other),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  3.  CSS TOKENIZER + PARSER
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum CssToken {
    Ident(String),
    Function(String),
    AtKeyword(String),
    Hash(String),
    StringToken(String),
    Number(f32),
    Percentage(f32),
    Dimension(f32, String),
    Whitespace,
    Colon,
    Semicolon,
    Comma,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Cdo,
    Cdc,
    Delim(char),
    Eof,
}

pub struct CssTokenizer {
    input: Vec<char>,
    pos: usize,
}

impl CssTokenizer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.input.get(self.pos).copied();
        if ch.is_some() {
            self.pos += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) -> bool {
        let start = self.pos;
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                ' ' | '\t' | '\n' | '\r' | '\x0C' => self.pos += 1,
                _ => break,
            }
        }
        self.pos > start
    }

    fn consume_ident(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c > '\x7F' {
                name.push(c);
                self.advance();
            } else {
                break;
            }
        }
        name
    }

    fn consume_string(&mut self, quote: char) -> String {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => break,
                Some(c) if c == quote => break,
                Some('\\') => {
                    if let Some(esc) = self.advance() {
                        match esc {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            _ => s.push(esc),
                        }
                    }
                }
                Some(c) => s.push(c),
            }
        }
        s
    }

    fn consume_number(&mut self) -> (f32, bool) {
        let start = self.pos;
        let mut has_dot = false;

        if self.peek() == Some('-') || self.peek() == Some('+') {
            self.advance();
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else if c == '.' && !has_dot {
                has_dot = true;
                self.advance();
            } else {
                break;
            }
        }
        let s: String = self.input[start..self.pos].iter().collect();
        (parse_f32(&s), has_dot)
    }

    fn skip_comment(&mut self) {
        while self.pos + 1 < self.input.len() {
            if self.input[self.pos] == '*' && self.input[self.pos + 1] == '/' {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
        self.pos = self.input.len();
    }

    pub fn next_token(&mut self) -> CssToken {
        if self.skip_whitespace() {
            return CssToken::Whitespace;
        }
        let ch = match self.advance() {
            None => return CssToken::Eof,
            Some(c) => c,
        };
        match ch {
            '/' if self.peek() == Some('*') => {
                self.advance();
                self.skip_comment();
                self.next_token()
            }
            ':' => CssToken::Colon,
            ';' => CssToken::Semicolon,
            ',' => CssToken::Comma,
            '{' => CssToken::LeftBrace,
            '}' => CssToken::RightBrace,
            '(' => CssToken::LeftParen,
            ')' => CssToken::RightParen,
            '[' => CssToken::LeftBracket,
            ']' => CssToken::RightBracket,
            '#' => {
                let name = self.consume_ident();
                CssToken::Hash(name)
            }
            '.' if self.peek().map_or(false, |c| c.is_ascii_digit()) => {
                self.pos -= 1;
                let (n, _) = self.consume_number();
                if self.peek() == Some('%') {
                    self.advance();
                    CssToken::Percentage(n)
                } else if self.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
                    let unit = self.consume_ident();
                    CssToken::Dimension(n, unit)
                } else {
                    CssToken::Number(n)
                }
            }
            '"' => CssToken::StringToken(self.consume_string('"')),
            '\'' => CssToken::StringToken(self.consume_string('\'')),
            '@' => {
                let name = self.consume_ident();
                CssToken::AtKeyword(name)
            }
            '<' if self.peek() == Some('!') => {
                if self.pos + 2 <= self.input.len()
                    && self.input[self.pos] == '!'
                    && self.input[self.pos + 1] == '-'
                {
                    self.pos += 2;
                    if self.pos < self.input.len() && self.input[self.pos] == '-' {
                        self.pos += 1;
                        return CssToken::Cdo;
                    }
                }
                CssToken::Delim('<')
            }
            '-' if self.peek() == Some('-') => {
                let next2 = self.input.get(self.pos + 1).copied();
                if next2 == Some('>') {
                    self.pos += 2;
                    CssToken::Cdc
                } else if self
                    .peek()
                    .map_or(false, |c| c.is_ascii_alphabetic() || c == '-')
                {
                    self.pos -= 1;
                    let name = self.consume_ident();
                    if self.peek() == Some('(') {
                        self.advance();
                        CssToken::Function(name)
                    } else {
                        CssToken::Ident(name)
                    }
                } else {
                    self.pos -= 1;
                    let (n, _) = self.consume_number();
                    CssToken::Number(n)
                }
            }
            c if c == '-' || c.is_ascii_digit() => {
                self.pos -= 1;
                if (c == '-' || c == '+')
                    && self
                        .input
                        .get(self.pos + 1)
                        .map_or(false, |n| n.is_ascii_alphabetic())
                {
                    let name = self.consume_ident();
                    if self.peek() == Some('(') {
                        self.advance();
                        CssToken::Function(name)
                    } else {
                        CssToken::Ident(name)
                    }
                } else if c == '-'
                    && !self
                        .input
                        .get(self.pos + 1)
                        .map_or(false, |n| n.is_ascii_digit() || *n == '.')
                {
                    // A standalone '-' (e.g. the subtraction operator inside calc()),
                    // not the sign of a number. Emit it as a delimiter so value
                    // parsers can see the operator. (`-5px` keeps the number branch.)
                    self.pos += 1;
                    CssToken::Delim('-')
                } else {
                    let (n, _) = self.consume_number();
                    if self.peek() == Some('%') {
                        self.advance();
                        CssToken::Percentage(n)
                    } else if self.peek().map_or(false, |c| c.is_ascii_alphabetic()) {
                        let unit = self.consume_ident();
                        CssToken::Dimension(n, unit)
                    } else {
                        CssToken::Number(n)
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' || c > '\x7F' => {
                self.pos -= 1;
                let name = self.consume_ident();
                if self.peek() == Some('(') {
                    self.advance();
                    CssToken::Function(name)
                } else {
                    CssToken::Ident(name)
                }
            }
            other => CssToken::Delim(other),
        }
    }

    pub fn tokenize_all(&mut self) -> Vec<CssToken> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            if tok == CssToken::Eof {
                tokens.push(tok);
                break;
            }
            tokens.push(tok);
        }
        tokens
    }
}

fn parse_f32(s: &str) -> f32 {
    let mut result: f64 = 0.0;
    let mut fraction = false;
    let mut frac_div: f64 = 1.0;
    let mut negative = false;
    let mut chars = s.chars();

    if let Some(first) = chars.next() {
        match first {
            '-' => negative = true,
            '+' => {}
            '.' => {
                fraction = true;
            }
            d if d.is_ascii_digit() => {
                result = (d as u32 - '0' as u32) as f64;
            }
            _ => return 0.0,
        }
    }

    for c in chars {
        if c == '.' {
            fraction = true;
            continue;
        }
        if let Some(d) = c.to_digit(10) {
            if fraction {
                frac_div *= 10.0;
                result += d as f64 / frac_div;
            } else {
                result = result * 10.0 + d as f64;
            }
        }
    }
    if negative {
        result = -result;
    }
    result as f32
}

// ── CSS Selectors ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum CssSelector {
    Universal,
    Type(String),
    Class(String),
    Id(String),
    Attribute {
        name: String,
        op: AttrOp,
        value: Option<String>,
    },
    PseudoClass(PseudoClass),
    Compound(Vec<CssSelector>),
    Combinator {
        left: Box<CssSelector>,
        combinator: CssCombinator,
        right: Box<CssSelector>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrOp {
    Exists,
    Equals,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PseudoClass {
    Hover,
    Focus,
    Active,
    FirstChild,
    LastChild,
    NthChild(i32),
    Not(Box<CssSelector>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssCombinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity(pub u32, pub u32, pub u32);

impl Specificity {
    pub fn zero() -> Self {
        Self(0, 0, 0)
    }

    pub fn value(&self) -> u32 {
        self.0 * 10000 + self.1 * 100 + self.2
    }
}

pub fn selector_specificity(sel: &CssSelector) -> Specificity {
    match sel {
        CssSelector::Universal => Specificity(0, 0, 0),
        CssSelector::Type(_) => Specificity(0, 0, 1),
        CssSelector::Class(_) => Specificity(0, 1, 0),
        CssSelector::Id(_) => Specificity(1, 0, 0),
        CssSelector::Attribute { .. } => Specificity(0, 1, 0),
        CssSelector::PseudoClass(pc) => match pc {
            PseudoClass::Not(inner) => selector_specificity(inner),
            _ => Specificity(0, 1, 0),
        },
        CssSelector::Compound(parts) => {
            let mut s = Specificity(0, 0, 0);
            for p in parts {
                let ps = selector_specificity(p);
                s.0 += ps.0;
                s.1 += ps.1;
                s.2 += ps.2;
            }
            s
        }
        CssSelector::Combinator { left, right, .. } => {
            let l = selector_specificity(left);
            let r = selector_specificity(right);
            Specificity(l.0 + r.0, l.1 + r.1, l.2 + r.2)
        }
    }
}

fn parse_single_selector(input: &str) -> Option<CssSelector> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut parts: Vec<CssSelector> = Vec::new();

    while pos < chars.len() {
        match chars[pos] {
            '*' => {
                parts.push(CssSelector::Universal);
                pos += 1;
            }
            '#' => {
                pos += 1;
                let start = pos;
                while pos < chars.len()
                    && (chars[pos].is_ascii_alphanumeric()
                        || chars[pos] == '-'
                        || chars[pos] == '_')
                {
                    pos += 1;
                }
                let id: String = chars[start..pos].iter().collect();
                parts.push(CssSelector::Id(id));
            }
            '.' => {
                pos += 1;
                let start = pos;
                while pos < chars.len()
                    && (chars[pos].is_ascii_alphanumeric()
                        || chars[pos] == '-'
                        || chars[pos] == '_')
                {
                    pos += 1;
                }
                let cls: String = chars[start..pos].iter().collect();
                parts.push(CssSelector::Class(cls));
            }
            '[' => {
                pos += 1;
                let start = pos;
                while pos < chars.len() && chars[pos] != ']' && chars[pos] != '=' {
                    pos += 1;
                }
                let attr_name: String = chars[start..pos].iter().collect();
                let attr_name = attr_name.trim().to_string();

                if pos < chars.len() && chars[pos] == '=' {
                    pos += 1;
                    let val_start = pos;
                    if pos < chars.len() && (chars[pos] == '"' || chars[pos] == '\'') {
                        let q = chars[pos];
                        pos += 1;
                        let vs = pos;
                        while pos < chars.len() && chars[pos] != q {
                            pos += 1;
                        }
                        let val: String = chars[vs..pos].iter().collect();
                        if pos < chars.len() {
                            pos += 1;
                        }
                        parts.push(CssSelector::Attribute {
                            name: attr_name,
                            op: AttrOp::Equals,
                            value: Some(val),
                        });
                    } else {
                        while pos < chars.len() && chars[pos] != ']' {
                            pos += 1;
                        }
                        let val: String = chars[val_start..pos].iter().collect();
                        parts.push(CssSelector::Attribute {
                            name: attr_name,
                            op: AttrOp::Equals,
                            value: Some(val.trim().to_string()),
                        });
                    }
                } else {
                    parts.push(CssSelector::Attribute {
                        name: attr_name,
                        op: AttrOp::Exists,
                        value: None,
                    });
                }
                if pos < chars.len() && chars[pos] == ']' {
                    pos += 1;
                }
            }
            ':' => {
                pos += 1;
                let is_pseudo_element = pos < chars.len() && chars[pos] == ':';
                if is_pseudo_element {
                    pos += 1;
                }
                let start = pos;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '-')
                {
                    pos += 1;
                }
                let pseudo_name: String = chars[start..pos].iter().collect();

                if pos < chars.len() && chars[pos] == '(' {
                    pos += 1;
                    let arg_start = pos;
                    let mut depth = 1;
                    while pos < chars.len() && depth > 0 {
                        if chars[pos] == '(' {
                            depth += 1;
                        } else if chars[pos] == ')' {
                            depth -= 1;
                        }
                        if depth > 0 {
                            pos += 1;
                        }
                    }
                    let arg: String = chars[arg_start..pos].iter().collect();
                    if pos < chars.len() {
                        pos += 1;
                    }
                    match pseudo_name.as_str() {
                        "nth-child" => {
                            let n = parse_f32(arg.trim()) as i32;
                            parts.push(CssSelector::PseudoClass(PseudoClass::NthChild(n)));
                        }
                        "not" => {
                            if let Some(inner) = parse_single_selector(&arg) {
                                parts.push(CssSelector::PseudoClass(PseudoClass::Not(Box::new(
                                    inner,
                                ))));
                            }
                        }
                        _ => {}
                    }
                } else {
                    let pc = match pseudo_name.as_str() {
                        "hover" => Some(PseudoClass::Hover),
                        "focus" => Some(PseudoClass::Focus),
                        "active" => Some(PseudoClass::Active),
                        "first-child" => Some(PseudoClass::FirstChild),
                        "last-child" => Some(PseudoClass::LastChild),
                        _ => None,
                    };
                    if let Some(pc) = pc {
                        parts.push(CssSelector::PseudoClass(pc));
                    }
                }
            }
            ' ' => {
                while pos < chars.len() && chars[pos] == ' ' {
                    pos += 1;
                }
                if pos < chars.len() {
                    if chars[pos] == '>' {
                        pos += 1;
                        while pos < chars.len() && chars[pos] == ' ' {
                            pos += 1;
                        }
                        let left = combine_parts(parts);
                        let rest: String = chars[pos..].iter().collect();
                        if let Some(right) = parse_single_selector(&rest) {
                            return Some(CssSelector::Combinator {
                                left: Box::new(left),
                                combinator: CssCombinator::Child,
                                right: Box::new(right),
                            });
                        }
                        return Some(left);
                    } else if chars[pos] == '+' {
                        pos += 1;
                        while pos < chars.len() && chars[pos] == ' ' {
                            pos += 1;
                        }
                        let left = combine_parts(parts);
                        let rest: String = chars[pos..].iter().collect();
                        if let Some(right) = parse_single_selector(&rest) {
                            return Some(CssSelector::Combinator {
                                left: Box::new(left),
                                combinator: CssCombinator::AdjacentSibling,
                                right: Box::new(right),
                            });
                        }
                        return Some(left);
                    } else if chars[pos] == '~' {
                        pos += 1;
                        while pos < chars.len() && chars[pos] == ' ' {
                            pos += 1;
                        }
                        let left = combine_parts(parts);
                        let rest: String = chars[pos..].iter().collect();
                        if let Some(right) = parse_single_selector(&rest) {
                            return Some(CssSelector::Combinator {
                                left: Box::new(left),
                                combinator: CssCombinator::GeneralSibling,
                                right: Box::new(right),
                            });
                        }
                        return Some(left);
                    } else {
                        let left = combine_parts(parts);
                        let rest: String = chars[pos..].iter().collect();
                        if let Some(right) = parse_single_selector(&rest) {
                            return Some(CssSelector::Combinator {
                                left: Box::new(left),
                                combinator: CssCombinator::Descendant,
                                right: Box::new(right),
                            });
                        }
                        return Some(left);
                    }
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = pos;
                while pos < chars.len()
                    && (chars[pos].is_ascii_alphanumeric()
                        || chars[pos] == '-'
                        || chars[pos] == '_')
                {
                    pos += 1;
                }
                let tag: String = chars[start..pos].iter().collect();
                parts.push(CssSelector::Type(tag));
            }
            _ => {
                pos += 1;
            }
        }
    }

    Some(combine_parts(parts))
}

fn combine_parts(parts: Vec<CssSelector>) -> CssSelector {
    match parts.len() {
        0 => CssSelector::Universal,
        1 => parts.into_iter().next().unwrap(),
        _ => CssSelector::Compound(parts),
    }
}

// ── CSS Stylesheet Parser ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CssStylesheet {
    pub rules: Vec<CssRule>,
}

#[derive(Debug, Clone)]
pub enum CssRule {
    Style(StyleRule),
    Media(MediaRule),
}

#[derive(Debug, Clone)]
pub struct StyleRule {
    pub selectors: Vec<CssSelector>,
    pub declarations: Vec<CssDeclaration>,
}

#[derive(Debug, Clone)]
pub struct CssDeclaration {
    pub property: String,
    pub value: CssValue,
    pub important: bool,
}

#[derive(Debug, Clone)]
pub struct MediaRule {
    pub query: MediaQuery,
    pub rules: Vec<StyleRule>,
}

#[derive(Debug, Clone)]
pub struct MediaQuery {
    pub media_type: String,
    pub min_width: Option<f32>,
    pub max_width: Option<f32>,
}

impl MediaQuery {
    /// Does this query apply at the given viewport width? Covers the parsed feature
    /// set (`min-width` / `max-width`) and the media type: `all`/`screen`/empty match
    /// (raeweb renders to a screen), everything else (e.g. `print`) does not.
    pub fn matches(&self, viewport_width: f32) -> bool {
        if !matches!(self.media_type.as_str(), "all" | "screen" | "") {
            return false;
        }
        if let Some(min) = self.min_width {
            if viewport_width < min {
                return false;
            }
        }
        if let Some(max) = self.max_width {
            if viewport_width > max {
                return false;
            }
        }
        true
    }
}

pub fn parse_css(input: &str) -> CssStylesheet {
    let mut tokenizer = CssTokenizer::new(input);
    let tokens = tokenizer.tokenize_all();
    let mut parser = CssParser::new(tokens);
    let mut sheet = parser.parse_stylesheet();
    resolve_custom_properties(&mut sheet);
    sheet
}

/// Resolve CSS custom properties (`--name`) and `var()` references after parsing.
///
/// raeweb resolves against a single GLOBAL custom-property pool (last declaration
/// wins) rather than the spec's per-element scoped cascade — this handles the
/// dominant real-world pattern (design tokens defined once on `:root` and read
/// everywhere via `var(--token)`) while staying simple and allocation-bounded.
/// `var(--x, fallback)` with an undefined `--x` resolves to the fallback; an
/// otherwise-unresolvable `var()` becomes `CssValue::None` (ignored, never
/// mis-rendered). Inline `style=""` var() is out of scope (parsed in isolation).
fn resolve_custom_properties(sheet: &mut CssStylesheet) {
    let mut vars: alloc::collections::BTreeMap<String, CssValue> =
        alloc::collections::BTreeMap::new();
    for rule in &sheet.rules {
        match rule {
            CssRule::Style(s) => collect_custom_props(&s.declarations, &mut vars),
            CssRule::Media(m) => {
                for s in &m.rules {
                    collect_custom_props(&s.declarations, &mut vars);
                }
            }
        }
    }
    // NB: do NOT skip when `vars` is empty — `var(--x, fallback)` must still
    // resolve to its fallback even with no custom properties defined. The
    // `value_has_var` pre-check keeps the no-var() common case cheap.
    for rule in &mut sheet.rules {
        match rule {
            CssRule::Style(s) => resolve_decl_vars(&mut s.declarations, &vars),
            CssRule::Media(m) => {
                for s in &mut m.rules {
                    resolve_decl_vars(&mut s.declarations, &vars);
                }
            }
        }
    }
}

fn collect_custom_props(
    decls: &[CssDeclaration],
    vars: &mut alloc::collections::BTreeMap<String, CssValue>,
) {
    for d in decls {
        if d.property.starts_with("--") {
            vars.insert(d.property.clone(), d.value.clone());
        }
    }
}

fn resolve_decl_vars(
    decls: &mut [CssDeclaration],
    vars: &alloc::collections::BTreeMap<String, CssValue>,
) {
    for d in decls {
        // Leave custom-property *definitions* intact; only rewrite consumers.
        if d.property.starts_with("--") {
            continue;
        }
        if value_has_var(&d.value) {
            d.value = resolve_var_value(&d.value, vars, 0);
        }
    }
}

/// Cheap pre-check so we only rebuild values that actually reference `var()`.
fn value_has_var(v: &CssValue) -> bool {
    match v {
        CssValue::Function(name, args) => name == "var" || args.iter().any(value_has_var),
        CssValue::Multiple(items) => items.iter().any(value_has_var),
        _ => false,
    }
}

fn resolve_var_value(
    v: &CssValue,
    vars: &alloc::collections::BTreeMap<String, CssValue>,
    depth: u8,
) -> CssValue {
    if depth > 16 {
        return CssValue::None; // cycle / pathological-nesting guard
    }
    match v {
        CssValue::Function(name, args) if name == "var" => {
            if let Some(CssValue::Keyword(var_name)) = args.first() {
                if let Some(found) = vars.get(var_name) {
                    return resolve_var_value(found, vars, depth + 1);
                }
            }
            // Undefined custom property: fall back to var()'s 2nd arg, else None.
            match args.get(1) {
                Some(fb) => resolve_var_value(fb, vars, depth + 1),
                None => CssValue::None,
            }
        }
        CssValue::Multiple(items) => CssValue::Multiple(
            items
                .iter()
                .map(|x| resolve_var_value(x, vars, depth))
                .collect(),
        ),
        CssValue::Function(name, args) => CssValue::Function(
            name.clone(),
            args.iter()
                .map(|x| resolve_var_value(x, vars, depth))
                .collect(),
        ),
        other => other.clone(),
    }
}

struct CssParser {
    tokens: Vec<CssToken>,
    pos: usize,
}

impl CssParser {
    fn new(tokens: Vec<CssToken>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &CssToken {
        self.tokens.get(self.pos).unwrap_or(&CssToken::Eof)
    }

    fn advance(&mut self) -> CssToken {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(CssToken::Eof);
        self.pos += 1;
        tok
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), CssToken::Whitespace) {
            self.pos += 1;
        }
    }

    fn parse_stylesheet(&mut self) -> CssStylesheet {
        let mut rules = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), CssToken::Eof) {
                break;
            }
            if let CssToken::AtKeyword(ref kw) = self.peek().clone() {
                if kw == "media" {
                    if let Some(mr) = self.parse_media_rule() {
                        rules.push(CssRule::Media(mr));
                    }
                    continue;
                } else {
                    self.skip_until_next_rule();
                    continue;
                }
            }
            if let Some(rule) = self.parse_style_rule() {
                rules.push(CssRule::Style(rule));
            }
        }
        CssStylesheet { rules }
    }

    fn parse_media_rule(&mut self) -> Option<MediaRule> {
        self.advance(); // @media
        self.skip_ws();
        let query = self.parse_media_query();
        self.skip_ws();
        if !matches!(self.peek(), CssToken::LeftBrace) {
            self.skip_until_next_rule();
            return None;
        }
        self.advance(); // {
        let mut rules = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), CssToken::RightBrace | CssToken::Eof) {
                break;
            }
            if let Some(rule) = self.parse_style_rule() {
                rules.push(rule);
            }
        }
        if matches!(self.peek(), CssToken::RightBrace) {
            self.advance();
        }
        Some(MediaRule { query, rules })
    }

    fn parse_media_query(&mut self) -> MediaQuery {
        let mut mq = MediaQuery {
            media_type: String::from("all"),
            min_width: None,
            max_width: None,
        };
        loop {
            self.skip_ws();
            match self.peek() {
                CssToken::LeftBrace | CssToken::Eof => break,
                CssToken::LeftParen => {
                    self.advance();
                    self.skip_ws();
                    if let CssToken::Ident(ref prop) = self.peek().clone() {
                        let prop = prop.clone();
                        self.advance();
                        self.skip_ws();
                        if matches!(self.peek(), CssToken::Colon) {
                            self.advance();
                            self.skip_ws();
                            let val = self.parse_dimension_value();
                            if prop == "min-width" {
                                mq.min_width = Some(val);
                            } else if prop == "max-width" {
                                mq.max_width = Some(val);
                            }
                        }
                    }
                    while !matches!(self.peek(), CssToken::RightParen | CssToken::Eof) {
                        self.advance();
                    }
                    if matches!(self.peek(), CssToken::RightParen) {
                        self.advance();
                    }
                }
                CssToken::Ident(ref mt) => {
                    mq.media_type = mt.clone();
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
        mq
    }

    fn parse_dimension_value(&mut self) -> f32 {
        match self.peek().clone() {
            CssToken::Dimension(n, _) => {
                self.advance();
                n
            }
            CssToken::Number(n) => {
                self.advance();
                n
            }
            _ => 0.0,
        }
    }

    fn parse_style_rule(&mut self) -> Option<StyleRule> {
        let selectors = self.parse_selector_list();
        self.skip_ws();
        if !matches!(self.peek(), CssToken::LeftBrace) {
            self.skip_until_next_rule();
            return None;
        }
        self.advance(); // {
        let declarations = self.parse_declarations();
        if matches!(self.peek(), CssToken::RightBrace) {
            self.advance();
        }
        Some(StyleRule {
            selectors,
            declarations,
        })
    }

    fn parse_selector_list(&mut self) -> Vec<CssSelector> {
        let mut selectors = Vec::new();
        let mut current = String::new();

        loop {
            match self.peek() {
                CssToken::LeftBrace | CssToken::Eof => break,
                CssToken::Comma => {
                    self.advance();
                    if let Some(sel) = parse_single_selector(current.trim()) {
                        selectors.push(sel);
                    }
                    current.clear();
                }
                _ => {
                    let tok = self.advance();
                    css_token_to_string(&tok, &mut current);
                }
            }
        }
        if !current.trim().is_empty() {
            if let Some(sel) = parse_single_selector(current.trim()) {
                selectors.push(sel);
            }
        }
        selectors
    }

    fn parse_declarations(&mut self) -> Vec<CssDeclaration> {
        let mut decls = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), CssToken::RightBrace | CssToken::Eof) {
                break;
            }
            if let Some(decl) = self.parse_declaration() {
                decls.push(decl);
            }
        }
        decls
    }

    fn parse_declaration(&mut self) -> Option<CssDeclaration> {
        self.skip_ws();
        let property = match self.peek().clone() {
            CssToken::Ident(name) => {
                self.advance();
                name
            }
            _ => {
                self.advance();
                return None;
            }
        };
        self.skip_ws();
        if !matches!(self.peek(), CssToken::Colon) {
            self.skip_until_semicolon();
            return None;
        }
        self.advance(); // :
        self.skip_ws();

        let mut value_parts = Vec::new();
        let mut important = false;
        loop {
            match self.peek() {
                CssToken::Semicolon => {
                    self.advance();
                    break;
                }
                CssToken::RightBrace | CssToken::Eof => break,
                _ => {
                    let tok = self.advance();
                    if let CssToken::Delim('!') = &tok {
                        self.skip_ws();
                        if let CssToken::Ident(ref s) = self.peek() {
                            if s == "important" {
                                important = true;
                                self.advance();
                                continue;
                            }
                        }
                    }
                    value_parts.push(tok);
                }
            }
        }

        let value = css_value_from_tokens(&value_parts);
        Some(CssDeclaration {
            property,
            value,
            important,
        })
    }

    fn skip_until_semicolon(&mut self) {
        loop {
            match self.peek() {
                CssToken::Semicolon => {
                    self.advance();
                    break;
                }
                CssToken::RightBrace | CssToken::Eof => break,
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn skip_until_next_rule(&mut self) {
        let mut depth = 0;
        loop {
            match self.peek() {
                CssToken::LeftBrace => {
                    depth += 1;
                    self.advance();
                }
                CssToken::RightBrace => {
                    if depth <= 1 {
                        self.advance();
                        break;
                    }
                    depth -= 1;
                    self.advance();
                }
                // A statement at-rule (`@import "x";`, `@charset`, `@namespace`)
                // ends at the first ';' before any block — without this the skip ran
                // on and swallowed the following rule.
                CssToken::Semicolon if depth == 0 => {
                    self.advance();
                    break;
                }
                CssToken::Eof => break,
                _ => {
                    self.advance();
                }
            }
        }
    }
}

fn css_token_to_string(tok: &CssToken, out: &mut String) {
    match tok {
        CssToken::Ident(s) => out.push_str(s),
        CssToken::Hash(s) => {
            out.push('#');
            out.push_str(s);
        }
        CssToken::Delim(c) => out.push(*c),
        CssToken::Whitespace => out.push(' '),
        CssToken::Colon => out.push(':'),
        CssToken::Comma => out.push(','),
        CssToken::LeftBracket => out.push('['),
        CssToken::RightBracket => out.push(']'),
        CssToken::LeftParen => out.push('('),
        CssToken::RightParen => out.push(')'),
        CssToken::Number(n) => {
            let mut buf = String::new();
            write_f32(*n, &mut buf);
            out.push_str(&buf);
        }
        CssToken::StringToken(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        CssToken::Function(s) => {
            out.push_str(s);
            out.push('(');
        }
        _ => {}
    }
}

fn write_f32(v: f32, out: &mut String) {
    let i = v as i32;
    if (v - i as f32).abs() < 0.0001 {
        let mut buf = [0u8; 20];
        let s = format_i32(i, &mut buf);
        out.push_str(s);
    } else {
        let int_part = v as i32;
        let frac = ((v - int_part as f32).abs() * 100.0) as u32;
        let mut buf = [0u8; 20];
        let s = format_i32(int_part, &mut buf);
        out.push_str(s);
        out.push('.');
        if frac < 10 {
            out.push('0');
        }
        let mut buf2 = [0u8; 20];
        let fs = format_i32(frac as i32, &mut buf2);
        out.push_str(fs);
    }
}

fn format_i32(mut val: i32, buf: &mut [u8; 20]) -> &str {
    if val == 0 {
        return "0";
    }
    let negative = val < 0;
    if negative {
        val = -val;
    }
    let mut pos = 20;
    while val > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    if negative {
        pos -= 1;
        buf[pos] = b'-';
    }
    core::str::from_utf8(&buf[pos..]).unwrap_or("0")
}

// ═══════════════════════════════════════════════════════════════════════════
//  4.  CSS VALUES + STYLE RESOLUTION
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
pub enum CssValue {
    Keyword(String),
    Length(f32, LengthUnit),
    Percentage(f32),
    Color(CssColor),
    Number(f32),
    Auto,
    None,
    Inherit,
    Initial,
    Url(String),
    Multiple(Vec<CssValue>),
    Function(String, Vec<CssValue>),
    /// A `calc()` reduced to its linear `px + pct% + em·font-size + vw + vh` form.
    /// Resolved against the containing block + font-size + viewport at layout (see
    /// `resolve_dimension` / `to_px`). Only produced for the MIXED case; pure px /
    /// % / number fold to Length / Percentage / Number.
    Calc {
        px: f32,
        pct: f32,
        em: f32,
        vw: f32,
        vh: f32,
    },
    Raw(String),
}

impl CssValue {
    pub fn is_auto(&self) -> bool {
        matches!(self, CssValue::Auto)
    }

    pub fn is_none(&self) -> bool {
        matches!(self, CssValue::None)
    }

    pub fn to_px(&self, parent_font_size: f32, viewport_width: f32, viewport_height: f32) -> f32 {
        match self {
            CssValue::Length(v, unit) => match unit {
                LengthUnit::Px => *v,
                LengthUnit::Em => v * parent_font_size,
                LengthUnit::Rem => v * 16.0,
                LengthUnit::Vw => v * viewport_width / 100.0,
                LengthUnit::Vh => v * viewport_height / 100.0,
                LengthUnit::Pt => v * 1.333,
            },
            CssValue::Percentage(p) => p / 100.0,
            CssValue::Number(n) => *n,
            // No container here, so the percentage part is dropped (callers that
            // know the container -- resolve_dimension -- handle Calc fully). The
            // font-size/viewport-relative terms DO resolve from the args we have.
            CssValue::Calc { px, em, vw, vh, .. } => {
                px + em * parent_font_size
                    + vw * viewport_width / 100.0
                    + vh * viewport_height / 100.0
            }
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    Px,
    Em,
    Rem,
    Vw,
    Vh,
    Pt,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CssColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl CssColor {
    pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0.0);

    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Self::rgb(r, g, b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Self::rgba(r, g, b, a as f32 / 255.0))
            }
            _ => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "black" => Some(Self::rgb(0, 0, 0)),
            "white" => Some(Self::rgb(255, 255, 255)),
            "red" => Some(Self::rgb(255, 0, 0)),
            "green" => Some(Self::rgb(0, 128, 0)),
            "blue" => Some(Self::rgb(0, 0, 255)),
            "yellow" => Some(Self::rgb(255, 255, 0)),
            "cyan" | "aqua" => Some(Self::rgb(0, 255, 255)),
            "magenta" | "fuchsia" => Some(Self::rgb(255, 0, 255)),
            "gray" | "grey" => Some(Self::rgb(128, 128, 128)),
            "silver" => Some(Self::rgb(192, 192, 192)),
            "maroon" => Some(Self::rgb(128, 0, 0)),
            "olive" => Some(Self::rgb(128, 128, 0)),
            "navy" => Some(Self::rgb(0, 0, 128)),
            "teal" => Some(Self::rgb(0, 128, 128)),
            "purple" => Some(Self::rgb(128, 0, 128)),
            "orange" => Some(Self::rgb(255, 165, 0)),
            "pink" => Some(Self::rgb(255, 192, 203)),
            "brown" => Some(Self::rgb(165, 42, 42)),
            "coral" => Some(Self::rgb(255, 127, 80)),
            "crimson" => Some(Self::rgb(220, 20, 60)),
            "gold" => Some(Self::rgb(255, 215, 0)),
            "indigo" => Some(Self::rgb(75, 0, 130)),
            "ivory" => Some(Self::rgb(255, 255, 240)),
            "khaki" => Some(Self::rgb(240, 230, 140)),
            "lime" => Some(Self::rgb(0, 255, 0)),
            "linen" => Some(Self::rgb(250, 240, 230)),
            "plum" => Some(Self::rgb(221, 160, 221)),
            "salmon" => Some(Self::rgb(250, 128, 114)),
            "sienna" => Some(Self::rgb(160, 82, 45)),
            "tan" => Some(Self::rgb(210, 180, 140)),
            "tomato" => Some(Self::rgb(255, 99, 71)),
            "turquoise" => Some(Self::rgb(64, 224, 208)),
            "violet" => Some(Self::rgb(238, 130, 238)),
            "wheat" => Some(Self::rgb(245, 222, 179)),
            "transparent" => Some(Self::TRANSPARENT),
            _ => None,
        }
    }
}

/// HSL→RGB per CSS Color 3 (`h` in degrees, `s`/`l` in 0..1). `no_std`-safe: no
/// `fmod` / `round` / transcendentals — hue is range-reduced with integer
/// truncation and channels are rounded via `+0.5` then clamped, so it links on
/// the bare target without libm.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    // Range-reduce hue into [0, 360) without fmod (libm-free), guarding NaN/∞.
    let mut hue = if h.is_finite() { h } else { 0.0 };
    hue -= (hue / 360.0) as i64 as f32 * 360.0;
    if hue < 0.0 {
        hue += 360.0;
    }
    let hue = hue / 360.0; // 0..1
    if s <= 0.0 {
        let v = unit_to_u8(l);
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    (
        unit_to_u8(hue_channel(p, q, hue + 1.0 / 3.0)),
        unit_to_u8(hue_channel(p, q, hue)),
        unit_to_u8(hue_channel(p, q, hue - 1.0 / 3.0)),
    )
}
fn hue_channel(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 1.0 / 2.0 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}
/// Round a 0..1 channel to a 0..255 byte without `f32::round` (libm-free).
fn unit_to_u8(x: f32) -> u8 {
    let v = x * 255.0 + 0.5;
    if v <= 0.0 {
        0
    } else if v >= 255.0 {
        255
    } else {
        v as u8
    }
}

/// A `calc()` intermediate: either a pure number or a linear combination of
/// length units that can only be resolved at layout time —
/// `px + pct% + em·font-size + vw·vw/100 + vh·vh/100`. (`rem` and `pt` are
/// constants, so they fold straight into `px` during evaluation.)
#[derive(Clone, Copy)]
enum CalcVal {
    Num(f32),
    Dim {
        px: f32,
        pct: f32,
        em: f32,
        vw: f32,
        vh: f32,
    },
}

/// Evaluate a `calc()` sub-expression from whitespace-filtered `toks[i..]`.
/// Grammar: expr = term (('+'|'-') term)* ; term = factor (('*'|'/') factor)* ;
/// factor = NUMBER | <px> | <%> | '(' expr ')'. Returns (value, index past it), or
/// None on a malformed expression or an unsupported unit (em/rem/vw/...), in which
/// case the caller leaves the calc() unevaluated.
fn calc_eval_expr(toks: &[&CssToken], i: usize) -> Option<(CalcVal, usize)> {
    let (mut left, mut i) = calc_eval_term(toks, i)?;
    while let Some(op) = toks.get(i).copied() {
        let sub = match op {
            CssToken::Delim('+') => false,
            CssToken::Delim('-') => true,
            _ => break,
        };
        let (right, ni) = calc_eval_term(toks, i + 1)?;
        left = calc_add(left, right, sub)?;
        i = ni;
    }
    Some((left, i))
}

fn calc_eval_term(toks: &[&CssToken], i: usize) -> Option<(CalcVal, usize)> {
    let (mut left, mut i) = calc_eval_factor(toks, i)?;
    while let Some(op) = toks.get(i).copied() {
        let div = match op {
            CssToken::Delim('*') => false,
            CssToken::Delim('/') => true,
            _ => break,
        };
        let (right, ni) = calc_eval_factor(toks, i + 1)?;
        left = if div {
            calc_div(left, right)?
        } else {
            calc_mul(left, right)?
        };
        i = ni;
    }
    Some((left, i))
}

fn calc_eval_factor(toks: &[&CssToken], i: usize) -> Option<(CalcVal, usize)> {
    let dim = |px: f32, pct: f32, em: f32, vw: f32, vh: f32| CalcVal::Dim {
        px,
        pct,
        em,
        vw,
        vh,
    };
    match toks.get(i).copied()? {
        CssToken::Number(n) => Some((CalcVal::Num(*n), i + 1)),
        CssToken::Percentage(p) => Some((dim(0.0, *p, 0.0, 0.0, 0.0), i + 1)),
        CssToken::Dimension(n, unit) => {
            // `rem`/`pt` are layout-independent constants → fold to px now.
            // `em`/`vw`/`vh` need font-size/viewport at layout → keep as terms.
            let v = match unit.as_str() {
                "px" => dim(*n, 0.0, 0.0, 0.0, 0.0),
                "rem" => dim(*n * 16.0, 0.0, 0.0, 0.0, 0.0),
                "pt" => dim(*n * 1.333, 0.0, 0.0, 0.0, 0.0),
                "em" => dim(0.0, 0.0, *n, 0.0, 0.0),
                "vw" => dim(0.0, 0.0, 0.0, *n, 0.0),
                "vh" => dim(0.0, 0.0, 0.0, 0.0, *n),
                _ => return None, // unsupported unit -> leave calc() unevaluated
            };
            Some((v, i + 1))
        }
        CssToken::LeftParen => {
            let (v, ni) = calc_eval_expr(toks, i + 1)?;
            match toks.get(ni).copied()? {
                CssToken::RightParen => Some((v, ni + 1)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn calc_add(a: CalcVal, b: CalcVal, sub: bool) -> Option<CalcVal> {
    let s = if sub { -1.0 } else { 1.0 };
    match (a, b) {
        (CalcVal::Num(x), CalcVal::Num(y)) => Some(CalcVal::Num(x + s * y)),
        (
            CalcVal::Dim {
                px: p1,
                pct: q1,
                em: e1,
                vw: w1,
                vh: h1,
            },
            CalcVal::Dim {
                px: p2,
                pct: q2,
                em: e2,
                vw: w2,
                vh: h2,
            },
        ) => Some(CalcVal::Dim {
            px: p1 + s * p2,
            pct: q1 + s * q2,
            em: e1 + s * e2,
            vw: w1 + s * w2,
            vh: h1 + s * h2,
        }),
        _ => None, // adding a number to a length is invalid in calc
    }
}

fn calc_mul(a: CalcVal, b: CalcVal) -> Option<CalcVal> {
    match (a, b) {
        (CalcVal::Num(x), CalcVal::Num(y)) => Some(CalcVal::Num(x * y)),
        (
            CalcVal::Num(x),
            CalcVal::Dim {
                px,
                pct,
                em,
                vw,
                vh,
            },
        )
        | (
            CalcVal::Dim {
                px,
                pct,
                em,
                vw,
                vh,
            },
            CalcVal::Num(x),
        ) => Some(CalcVal::Dim {
            px: px * x,
            pct: pct * x,
            em: em * x,
            vw: vw * x,
            vh: vh * x,
        }),
        _ => None, // length * length is invalid
    }
}

fn calc_div(a: CalcVal, b: CalcVal) -> Option<CalcVal> {
    match (a, b) {
        (CalcVal::Num(x), CalcVal::Num(y)) if y != 0.0 => Some(CalcVal::Num(x / y)),
        (
            CalcVal::Dim {
                px,
                pct,
                em,
                vw,
                vh,
            },
            CalcVal::Num(y),
        ) if y != 0.0 => Some(CalcVal::Dim {
            px: px / y,
            pct: pct / y,
            em: em / y,
            vw: vw / y,
            vh: vh / y,
        }),
        _ => None,
    }
}

/// Parse the comma-separated args of a function call whose `(` was just consumed
/// (so `i` points at the first arg token). Recurses into nested functions so
/// `rgb()`/`hsl()` resolve inside another function. Returns (args, index past the
/// matching `)`).
fn parse_func_args(toks: &[&CssToken], mut i: usize) -> (Vec<CssValue>, usize) {
    let mut args = Vec::new();
    while let Some(t) = toks.get(i).copied() {
        match t {
            CssToken::RightParen => {
                i += 1;
                break;
            }
            CssToken::Comma => {
                i += 1;
            }
            CssToken::Function(nm) => {
                let (nested, ni) = parse_func_args(toks, i + 1);
                args.push(func_value(nm, nested));
                i = ni;
            }
            other => {
                args.push(single_token_value(other));
                i += 1;
            }
        }
    }
    (args, i)
}

/// Convert a function name + its (already value-parsed) args to a CssValue:
/// `rgb()`/`rgba()`/`hsl()`/`hsla()` fold to a Color; anything else stays a Function.
fn func_value(name: &str, args: Vec<CssValue>) -> CssValue {
    if name == "rgb" || name == "rgba" {
        let nums: Vec<f32> = args
            .iter()
            .filter_map(|v| match v {
                CssValue::Number(n) => Some(*n),
                CssValue::Percentage(p) => Some(p * 2.55),
                _ => None,
            })
            .collect();
        if nums.len() >= 3 {
            let a = if nums.len() >= 4 { nums[3] } else { 1.0 };
            return CssValue::Color(CssColor::rgba(
                nums[0] as u8,
                nums[1] as u8,
                nums[2] as u8,
                a,
            ));
        }
    }
    if (name == "hsl" || name == "hsla") && args.len() >= 3 {
        let hue = match &args[0] {
            CssValue::Number(n) => Some(*n),
            CssValue::Length(n, _) => Some(*n),
            CssValue::Percentage(p) => Some(*p),
            _ => None,
        };
        let chan = |v: &CssValue| -> f32 {
            match v {
                CssValue::Percentage(p) => p / 100.0,
                CssValue::Number(n) => *n,
                _ => 0.0,
            }
        };
        if let Some(h) = hue {
            let sat = chan(&args[1]).clamp(0.0, 1.0);
            let light = chan(&args[2]).clamp(0.0, 1.0);
            let a = if args.len() >= 4 {
                chan(&args[3]).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let (r, g, b) = hsl_to_rgb(h, sat, light);
            return CssValue::Color(CssColor::rgba(r, g, b, a));
        }
    }
    CssValue::Function(name.to_string(), args)
}

/// Parse ONE value at `toks[i]`: a function call (recursively, consuming its args)
/// or a single token. Returns (value, index past it).
fn parse_value_token(toks: &[&CssToken], i: usize) -> (CssValue, usize) {
    if let Some(CssToken::Function(name)) = toks.get(i).copied() {
        let (args, ni) = parse_func_args(toks, i + 1);
        (func_value(name, args), ni)
    } else {
        let v = toks
            .get(i)
            .copied()
            .map(single_token_value)
            .unwrap_or(CssValue::None);
        (v, i + 1)
    }
}

fn css_value_from_tokens(tokens: &[CssToken]) -> CssValue {
    let tokens: Vec<&CssToken> = tokens
        .iter()
        .filter(|t| !matches!(t, CssToken::Whitespace))
        .collect();
    if tokens.is_empty() {
        return CssValue::None;
    }
    if tokens.len() == 1 {
        return single_token_value(tokens[0]);
    }

    if let CssToken::Function(ref name) = tokens[0] {
        if name == "calc" {
            if let Some((val, ni)) = calc_eval_expr(&tokens, 1) {
                if matches!(tokens.get(ni).copied(), Some(CssToken::RightParen) | None) {
                    return match val {
                        CalcVal::Num(n) => CssValue::Number(n),
                        CalcVal::Dim {
                            px,
                            pct,
                            em,
                            vw,
                            vh,
                        } => {
                            let only_px = pct == 0.0 && em == 0.0 && vw == 0.0 && vh == 0.0;
                            let only_pct = px == 0.0 && em == 0.0 && vw == 0.0 && vh == 0.0;
                            if only_px {
                                CssValue::Length(px, LengthUnit::Px)
                            } else if only_pct {
                                CssValue::Percentage(pct)
                            } else {
                                CssValue::Calc {
                                    px,
                                    pct,
                                    em,
                                    vw,
                                    vh,
                                }
                            }
                        }
                    };
                }
            }
            // malformed / unsupported unit -> fall through (leave it a Function).
        }
        // Parse the call's args recursively so NESTED functions resolve (e.g.
        // `linear-gradient(rgb(...), ...)` — previously the flat loop broke at the
        // inner `)` and lost the nested color).
        let (args, _) = parse_func_args(&tokens, 1);
        return func_value(name, args);
    }

    // Space-separated values; resolve nested functions (rgb/hsl) as single values
    // instead of leaving them inert (e.g. `border: 1px solid rgb(...)`,
    // `box-shadow: 0 0 4px rgb(...)`).
    let mut multi = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (v, ni) = parse_value_token(&tokens, i);
        multi.push(v);
        i = ni;
    }
    CssValue::Multiple(multi)
}

fn single_token_value(tok: &CssToken) -> CssValue {
    match tok {
        CssToken::Ident(s) => match s.as_str() {
            "auto" => CssValue::Auto,
            "none" => CssValue::None,
            "inherit" => CssValue::Inherit,
            "initial" => CssValue::Initial,
            _ => {
                if let Some(color) = CssColor::from_name(s) {
                    CssValue::Color(color)
                } else {
                    CssValue::Keyword(s.clone())
                }
            }
        },
        CssToken::Hash(h) => {
            if let Some(c) = CssColor::from_hex(h) {
                CssValue::Color(c)
            } else {
                CssValue::Raw(alloc::format!("#{}", h))
            }
        }
        CssToken::Number(n) => CssValue::Number(*n),
        CssToken::Percentage(p) => CssValue::Percentage(*p),
        CssToken::Dimension(n, unit) => {
            let lu = match unit.as_str() {
                "px" => LengthUnit::Px,
                "em" => LengthUnit::Em,
                "rem" => LengthUnit::Rem,
                "vw" => LengthUnit::Vw,
                "vh" => LengthUnit::Vh,
                "pt" => LengthUnit::Pt,
                _ => LengthUnit::Px,
            };
            CssValue::Length(*n, lu)
        }
        CssToken::StringToken(s) => CssValue::Raw(s.clone()),
        CssToken::Function(name) => {
            if name == "url" {
                CssValue::Url(String::new())
            } else {
                CssValue::Function(name.clone(), Vec::new())
            }
        }
        _ => CssValue::None,
    }
}

// ── Style Resolution ──────────────────────────────────────────────────────

const INHERITED_PROPERTIES: &[&str] = &[
    "color",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "line-height",
    "text-align",
    "text-decoration",
    "text-shadow",
    "text-transform",
    "letter-spacing",
    "word-spacing",
    "white-space",
    "visibility",
    "cursor",
    "list-style",
    "list-style-type",
    "list-style-position",
    "direction",
    "quotes",
];

fn is_inherited(property: &str) -> bool {
    INHERITED_PROPERTIES.contains(&property)
}

#[derive(Debug, Clone)]
pub struct ComputedStyle {
    pub properties: Vec<(String, CssValue)>,
}

impl ComputedStyle {
    pub fn new() -> Self {
        Self {
            properties: Vec::new(),
        }
    }

    pub fn get(&self, prop: &str) -> Option<&CssValue> {
        self.properties
            .iter()
            .rev()
            .find(|(k, _)| k == prop)
            .map(|(_, v)| v)
    }

    pub fn set(&mut self, prop: &str, val: CssValue) {
        if let Some(existing) = self.properties.iter_mut().find(|(k, _)| k == prop) {
            existing.1 = val;
        } else {
            self.properties.push((prop.to_string(), val));
        }
    }

    pub fn display(&self) -> DisplayMode {
        match self.get("display") {
            Some(CssValue::Keyword(k)) => match k.as_str() {
                "block" => DisplayMode::Block,
                "inline" => DisplayMode::Inline,
                "inline-block" => DisplayMode::InlineBlock,
                "flex" => DisplayMode::Flex,
                "grid" => DisplayMode::Grid,
                "list-item" => DisplayMode::ListItem,
                "none" => DisplayMode::None,
                _ => DisplayMode::Block,
            },
            Some(CssValue::None) => DisplayMode::None,
            _ => DisplayMode::Block,
        }
    }

    pub fn position(&self) -> Position {
        match self.get("position") {
            Some(CssValue::Keyword(k)) => match k.as_str() {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                "fixed" => Position::Fixed,
                "sticky" => Position::Sticky,
                _ => Position::Static,
            },
            _ => Position::Static,
        }
    }
}

fn default_styles_for_tag(tag: &str) -> Vec<(String, CssValue)> {
    let mut styles = Vec::new();
    match tag {
        "html" | "body" | "div" | "main" | "section" | "article" | "nav" | "aside" | "header"
        | "footer" | "form" | "fieldset" | "details" | "dialog" | "summary" | "address"
        | "figcaption" | "hgroup" | "search" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
        }
        "blockquote" | "figure" => {
            // Browsers indent these: `margin: 1em 40px`. Without it they render flush
            // like a plain <div>, losing the visual quote/figure offset.
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push((
                "margin".into(),
                CssValue::Multiple(vec![
                    CssValue::Length(16.0, LengthUnit::Px),
                    CssValue::Length(40.0, LengthUnit::Px),
                ]),
            ));
        }
        "h1" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(32.0, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(21.44, LengthUnit::Px)));
        }
        "h2" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(24.0, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(19.92, LengthUnit::Px)));
        }
        "h3" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(18.72, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(18.72, LengthUnit::Px)));
        }
        "h4" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(16.0, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(21.28, LengthUnit::Px)));
        }
        "h5" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(13.28, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(22.16, LengthUnit::Px)));
        }
        "h6" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("font-size".into(), CssValue::Length(10.72, LengthUnit::Px)));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
            styles.push(("margin".into(), CssValue::Length(24.97, LengthUnit::Px)));
        }
        "hr" => {
            // A thin horizontal rule: a 1px-tall block filled gray, spanning the
            // container. Browsers draw an inset border; we use the equivalent modern
            // height+background form so the rule is actually painted (not collapsed).
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("height".into(), CssValue::Length(1.0, LengthUnit::Px)));
            styles.push(("margin".into(), CssValue::Length(8.0, LengthUnit::Px)));
            styles.push((
                "background-color".into(),
                CssValue::Color(CssColor::rgb(128, 128, 128)),
            ));
        }
        "p" | "dl" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("margin".into(), CssValue::Length(16.0, LengthUnit::Px)));
        }
        "pre" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("margin".into(), CssValue::Length(16.0, LengthUnit::Px)));
            styles.push(("white-space".into(), CssValue::Keyword("pre".into())));
            styles.push(("font-family".into(), CssValue::Keyword("monospace".into())));
        }
        "strong" | "b" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
            styles.push(("font-weight".into(), CssValue::Keyword("bold".into())));
        }
        "em" | "i" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
            styles.push(("font-style".into(), CssValue::Keyword("italic".into())));
        }
        "a" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
            styles.push(("color".into(), CssValue::Color(CssColor::rgb(0, 0, 238))));
            styles.push((
                "text-decoration".into(),
                CssValue::Keyword("underline".into()),
            ));
            styles.push(("cursor".into(), CssValue::Keyword("pointer".into())));
        }
        "u" | "ins" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
            styles.push((
                "text-decoration".into(),
                CssValue::Keyword("underline".into()),
            ));
        }
        "s" | "del" | "strike" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
            styles.push((
                "text-decoration".into(),
                CssValue::Keyword("line-through".into()),
            ));
        }
        "span" | "small" | "sub" | "sup" | "mark" | "abbr" | "cite" | "code" | "kbd" | "samp"
        | "var" | "q" | "dfn" | "label" | "output" | "time" | "data" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
        }
        "ul" | "menu" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("margin".into(), CssValue::Length(16.0, LengthUnit::Px)));
            styles.push(("padding".into(), CssValue::Length(40.0, LengthUnit::Px)));
            styles.push(("list-style-type".into(), CssValue::Keyword("disc".into())));
        }
        "ol" => {
            styles.push(("display".into(), CssValue::Keyword("block".into())));
            styles.push(("margin".into(), CssValue::Length(16.0, LengthUnit::Px)));
            styles.push(("padding".into(), CssValue::Length(40.0, LengthUnit::Px)));
            styles.push((
                "list-style-type".into(),
                CssValue::Keyword("decimal".into()),
            ));
        }
        "li" => {
            styles.push(("display".into(), CssValue::Keyword("list-item".into())));
        }
        "table" => {
            styles.push(("display".into(), CssValue::Keyword("table".into())));
            styles.push((
                "border-collapse".into(),
                CssValue::Keyword("separate".into()),
            ));
        }
        "img" | "video" | "canvas" | "svg" => {
            styles.push(("display".into(), CssValue::Keyword("inline-block".into())));
        }
        "input" | "textarea" | "select" | "button" => {
            styles.push(("display".into(), CssValue::Keyword("inline-block".into())));
        }
        "br" => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
        }
        _ => {
            styles.push(("display".into(), CssValue::Keyword("inline".into())));
        }
    }
    styles
}

#[derive(Debug, Clone)]
struct MatchedDeclaration {
    property: String,
    value: CssValue,
    specificity: Specificity,
    _important: bool,
    source_order: u32,
    origin: CascadeOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CascadeOrigin {
    _UserAgent,
    Author,
    AuthorImportant,
    Inline,
}

pub fn resolve_styles(dom: &DomNode, stylesheet: &CssStylesheet) -> StyledNode {
    let mut order = 0u32;
    build_styled_tree(dom, stylesheet, None, &mut order)
}

pub struct StyledNode {
    pub node: DomNode,
    pub style: ComputedStyle,
    pub children: Vec<StyledNode>,
}

/// Flatten matching `@media` blocks into top-level style rules for `viewport_width`
/// (preserving document order, so the cascade is unchanged); non-matching blocks are
/// dropped. This lets the viewport-less styler apply responsive CSS without threading
/// a viewport through it. `var()`/custom properties are already resolved by
/// `parse_css`, so flattening afterwards keeps resolved values.
fn expand_media_rules(sheet: &CssStylesheet, viewport_width: f32) -> CssStylesheet {
    let mut rules: Vec<CssRule> = Vec::with_capacity(sheet.rules.len());
    for rule in &sheet.rules {
        match rule {
            CssRule::Style(s) => rules.push(CssRule::Style(s.clone())),
            CssRule::Media(m) => {
                if m.query.matches(viewport_width) {
                    for sr in &m.rules {
                        rules.push(CssRule::Style(sr.clone()));
                    }
                }
            }
        }
    }
    CssStylesheet { rules }
}

const BORDER_STYLE_KEYWORDS: &[&str] = &[
    "none", "hidden", "solid", "dashed", "dotted", "double", "groove", "ridge", "inset", "outset",
];

/// Apply one declaration to a computed style, expanding the `border` shorthand into
/// its longhands so `border: 1px solid red` actually sets border-width/-style/-color
/// (the layout/paint read the longhands). Done per-declaration in cascade order so a
/// later longhand still overrides an earlier shorthand and vice-versa.
fn apply_declaration(style: &mut ComputedStyle, prop: &str, value: &CssValue) {
    if prop == "border" {
        apply_border_shorthand(style, value);
        return;
    }
    if prop == "font" {
        apply_font_shorthand(style, value);
        return;
    }
    style.set(prop, value.clone());
}

/// Expand the `font` shorthand (`italic bold 16px Arial`) into font-style/-weight/
/// -size/-family longhands. The font-size is the pivot: tokens before it are
/// style/weight; tokens after it are the family. (line-height in `size/lh` is not
/// extracted -- uncommon; the longhands carry the rest.)
fn apply_font_shorthand(style: &mut ComputedStyle, value: &CssValue) {
    let single = [value.clone()];
    let parts: &[CssValue] = match value {
        CssValue::Multiple(items) => items.as_slice(),
        _ => &single,
    };
    let size_idx = match parts
        .iter()
        .position(|v| matches!(v, CssValue::Length(..) | CssValue::Percentage(_)))
    {
        Some(i) => i,
        None => return,
    };
    style.set("font-size", parts[size_idx].clone());
    for v in &parts[..size_idx] {
        match v {
            CssValue::Keyword(k) => match k.to_ascii_lowercase().as_str() {
                "italic" | "oblique" => style.set("font-style", v.clone()),
                "bold" | "bolder" | "lighter" | "normal" => style.set("font-weight", v.clone()),
                _ => {}
            },
            CssValue::Number(n) if *n >= 100.0 && *n <= 900.0 => {
                style.set("font-weight", v.clone());
            }
            _ => {}
        }
    }
    let names: alloc::vec::Vec<String> = parts[size_idx + 1..]
        .iter()
        .filter_map(|v| match v {
            CssValue::Keyword(k) => Some(k.clone()),
            CssValue::Raw(r) => Some(r.clone()),
            _ => None,
        })
        .collect();
    if !names.is_empty() {
        style.set("font-family", CssValue::Raw(names.join(", ")));
    }
}

fn apply_border_shorthand(style: &mut ComputedStyle, value: &CssValue) {
    let single = [value.clone()];
    let parts: &[CssValue] = match value {
        CssValue::Multiple(items) => items.as_slice(),
        _ => &single,
    };
    for v in parts {
        match v {
            CssValue::Length(..) | CssValue::Number(_) => {
                style.set("border-width", v.clone());
            }
            CssValue::Color(_) => style.set("border-color", v.clone()),
            CssValue::Keyword(k) if BORDER_STYLE_KEYWORDS.contains(&k.as_str()) => {
                style.set("border-style", v.clone());
            }
            _ => {}
        }
    }
}

/// Parse an HTML `width=`/`height=` attribute value -> a CSS length or percentage.
fn parse_html_dimension(s: &str) -> Option<CssValue> {
    let s = s.trim();
    if let Some(pct) = s.strip_suffix('%') {
        pct.trim().parse::<f32>().ok().map(CssValue::Percentage)
    } else {
        let num = s.strip_suffix("px").unwrap_or(s).trim();
        num.parse::<f32>()
            .ok()
            .map(|n| CssValue::Length(n, LengthUnit::Px))
    }
}

/// Parse an HTML color attribute (`bgcolor=`, `<font color>`): `#rgb`/`#rrggbb`, a
/// named color, or a bare hex string.
fn parse_color_attr(s: &str) -> Option<CssColor> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return CssColor::from_hex(hex);
    }
    if let Some(c) = CssColor::from_name(&s.to_ascii_lowercase()) {
        return Some(c);
    }
    CssColor::from_hex(s)
}

/// Map presentational HTML attributes to CSS on the element's style.
fn apply_presentational_attrs(dom: &DomNode, tag: &str, style: &mut ComputedStyle) {
    if matches!(
        tag,
        "img"
            | "table"
            | "td"
            | "th"
            | "col"
            | "colgroup"
            | "hr"
            | "canvas"
            | "video"
            | "iframe"
            | "embed"
            | "object"
    ) {
        if let Some(v) = dom.get_attribute("width").and_then(parse_html_dimension) {
            style.set("width", v);
        }
        if let Some(v) = dom.get_attribute("height").and_then(parse_html_dimension) {
            style.set("height", v);
        }
    }
    if matches!(tag, "body" | "table" | "td" | "th" | "tr") {
        if let Some(c) = dom.get_attribute("bgcolor").and_then(parse_color_attr) {
            style.set("background-color", CssValue::Color(c));
        }
    }
    if tag == "font" {
        if let Some(c) = dom.get_attribute("color").and_then(parse_color_attr) {
            style.set("color", CssValue::Color(c));
        }
    }
}

pub fn build_styled_tree(
    dom: &DomNode,
    stylesheet: &CssStylesheet,
    parent_style: Option<&ComputedStyle>,
    source_order: &mut u32,
) -> StyledNode {
    build_styled_tree_ctx(dom, stylesheet, parent_style, source_order, &[], &[])
}

/// `build_styled_tree` with the ancestor chain threaded so combinator selectors
/// (descendant / child) resolve correctly (see [`selector_matches_ctx`]).
fn build_styled_tree_ctx<'a>(
    dom: &'a DomNode,
    stylesheet: &CssStylesheet,
    parent_style: Option<&ComputedStyle>,
    source_order: &mut u32,
    ancestors: &[&'a DomNode],
    prev_siblings: &[&'a DomNode],
) -> StyledNode {
    let mut style = ComputedStyle::new();

    if let Some(parent) = parent_style {
        for (prop, val) in &parent.properties {
            if is_inherited(prop) {
                style.set(prop, val.clone());
            }
        }
    }

    if let Some(tag) = dom.tag_name() {
        for (prop, val) in default_styles_for_tag(tag) {
            style.set(&prop, val);
        }
        // Presentational HTML attributes (width=/height=/bgcolor=/<font color>) map to
        // low-priority CSS — applied here so author rules + inline styles override them.
        apply_presentational_attrs(dom, tag, &mut style);
    }

    let mut matched: Vec<MatchedDeclaration> = Vec::new();

    for rule in &stylesheet.rules {
        match rule {
            CssRule::Style(sr) => {
                for sel in &sr.selectors {
                    if selector_matches_ctx(dom, sel, ancestors, prev_siblings) {
                        let spec = selector_specificity(sel);
                        for decl in &sr.declarations {
                            *source_order += 1;
                            matched.push(MatchedDeclaration {
                                property: decl.property.clone(),
                                value: decl.value.clone(),
                                specificity: spec,
                                _important: decl.important,
                                source_order: *source_order,
                                origin: if decl.important {
                                    CascadeOrigin::AuthorImportant
                                } else {
                                    CascadeOrigin::Author
                                },
                            });
                        }
                    }
                }
            }
            CssRule::Media(_mr) => {}
        }
    }

    if let Some(inline_style) = dom.get_attribute("style") {
        let inline_css = alloc::format!("x {{ {} }}", inline_style);
        let parsed = parse_css(&inline_css);
        for rule in &parsed.rules {
            if let CssRule::Style(sr) = rule {
                for decl in &sr.declarations {
                    *source_order += 1;
                    matched.push(MatchedDeclaration {
                        property: decl.property.clone(),
                        value: decl.value.clone(),
                        specificity: Specificity(1, 0, 0),
                        _important: decl.important,
                        source_order: *source_order,
                        origin: CascadeOrigin::Inline,
                    });
                }
            }
        }
    }

    matched.sort_by(|a, b| {
        a.origin
            .cmp(&b.origin)
            .then(a.specificity.cmp(&b.specificity))
            .then(a.source_order.cmp(&b.source_order))
    });

    for md in &matched {
        match &md.value {
            // `inherit` resolves to the parent's computed value for this property
            // (the explicit form — e.g. `color: inherit` on a link to match its
            // container — which the default inherited-property copy alone misses for
            // non-inherited properties).
            CssValue::Inherit => {
                if let Some(v) = parent_style.and_then(|p| p.get(&md.property)).cloned() {
                    apply_declaration(&mut style, &md.property, &v);
                } else {
                    style.properties.retain(|(k, _)| k != &md.property);
                }
            }
            // `initial` resets the property to its initial value -> drop it so reads
            // fall through to the document default.
            CssValue::Initial => {
                style.properties.retain(|(k, _)| k != &md.property);
            }
            v => apply_declaration(&mut style, &md.property, v),
        }
    }

    let mut child_ancestors: Vec<&DomNode> = ancestors.to_vec();
    child_ancestors.push(dom);
    // Accumulate each child's preceding ELEMENT siblings (text nodes don't take part
    // in sibling combinators) so `h1 + p` / `h1 ~ p` resolve during styling.
    let mut prev_sibs: Vec<&DomNode> = Vec::new();
    let mut children: Vec<StyledNode> = Vec::with_capacity(dom.children.len());
    for child in &dom.children {
        children.push(build_styled_tree_ctx(
            child,
            stylesheet,
            Some(&style),
            source_order,
            &child_ancestors,
            &prev_sibs,
        ));
        if child.is_element() {
            prev_sibs.push(child);
        }
    }

    StyledNode {
        node: dom.clone(),
        style,
        children,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  5.  LAYOUT ENGINE
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayMode {
    Block,
    Inline,
    InlineBlock,
    Flex,
    Grid,
    None,
    ListItem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EdgeSizes {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl EdgeSizes {
    pub fn uniform(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains_point(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.width && py >= self.y && py <= self.y + self.height
    }

    pub fn expanded_by(&self, edge: &EdgeSizes) -> Rect {
        Rect {
            x: self.x - edge.left,
            y: self.y - edge.top,
            width: self.width + edge.horizontal(),
            height: self.height + edge.vertical(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BoxDimensions {
    pub content: Rect,
    pub padding: EdgeSizes,
    pub border: EdgeSizes,
    pub margin: EdgeSizes,
}

impl BoxDimensions {
    pub fn padding_box(&self) -> Rect {
        self.content.expanded_by(&self.padding)
    }

    pub fn border_box(&self) -> Rect {
        self.padding_box().expanded_by(&self.border)
    }

    pub fn margin_box(&self) -> Rect {
        self.border_box().expanded_by(&self.margin)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSide {
    Left,
    Right,
}

#[derive(Debug)]
pub struct LayoutBox {
    pub dimensions: BoxDimensions,
    pub display: DisplayMode,
    pub position: Position,
    pub tag_name: Option<String>,
    pub text: Option<String>,
    pub children: Vec<LayoutBox>,
    pub z_index: i32,
    pub float: Option<FloatSide>,
    pub opacity: f32,
    pub overflow_hidden: bool,
    pub node_id: Option<String>,
    /// 1-based ordinal among list-item siblings (for `<ol>` markers); 0 if not a
    /// list item.
    pub list_index: usize,
    pub style: ComputedStyle,
}

impl LayoutBox {
    pub fn new(display: DisplayMode) -> Self {
        Self {
            dimensions: BoxDimensions::default(),
            display,
            position: Position::Static,
            tag_name: None,
            text: None,
            children: Vec::new(),
            z_index: 0,
            float: None,
            opacity: 1.0,
            overflow_hidden: false,
            node_id: None,
            list_index: 0,
            style: ComputedStyle::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub width: f32,
    pub height: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}

impl Viewport {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            scroll_x: 0.0,
            scroll_y: 0.0,
        }
    }
}

fn resolve_edge(style: &ComputedStyle, prop: &str, vp: &Viewport) -> EdgeSizes {
    if let Some(val) = style.get(prop) {
        // A box-model shorthand may carry 1–4 space-separated values
        // (CssValue::Multiple). Expand per the CSS clockwise rules; a single value
        // (bare or one-element Multiple) applies to all four sides. Previously a
        // Multiple hit to_px()'s `_ => 0.0` arm, so `margin: 10px 20px` silently
        // zeroed ALL sides.
        if let CssValue::Multiple(items) = val {
            let px: Vec<f32> = items
                .iter()
                .map(|v| v.to_px(16.0, vp.width, vp.height))
                .collect();
            let (top, right, bottom, left) = match px.as_slice() {
                [a] => (*a, *a, *a, *a),
                [a, b] => (*a, *b, *a, *b),
                [a, b, c] => (*a, *b, *c, *b),
                [a, b, c, d, ..] => (*a, *b, *c, *d),
                [] => (0.0, 0.0, 0.0, 0.0),
            };
            return EdgeSizes {
                top,
                right,
                bottom,
                left,
            };
        }
        let v = val.to_px(16.0, vp.width, vp.height);
        return EdgeSizes::uniform(v);
    }
    let top = style
        .get(&alloc::format!("{}-top", prop))
        .map(|v| v.to_px(16.0, vp.width, vp.height))
        .unwrap_or(0.0);
    let right = style
        .get(&alloc::format!("{}-right", prop))
        .map(|v| v.to_px(16.0, vp.width, vp.height))
        .unwrap_or(0.0);
    let bottom = style
        .get(&alloc::format!("{}-bottom", prop))
        .map(|v| v.to_px(16.0, vp.width, vp.height))
        .unwrap_or(0.0);
    let left = style
        .get(&alloc::format!("{}-left", prop))
        .map(|v| v.to_px(16.0, vp.width, vp.height))
        .unwrap_or(0.0);
    EdgeSizes {
        top,
        right,
        bottom,
        left,
    }
}

fn resolve_dimension(
    style: &ComputedStyle,
    prop: &str,
    container: f32,
    vp: &Viewport,
) -> Option<f32> {
    match style.get(prop) {
        Some(CssValue::Auto) | None => None,
        Some(CssValue::Percentage(p)) => Some(container * p / 100.0),
        Some(CssValue::Calc {
            px,
            pct,
            em,
            vw,
            vh,
        }) => {
            let font_size = match style.get("font-size") {
                Some(val) => val.to_px(16.0, vp.width, vp.height),
                None => 16.0,
            };
            Some(
                px + container * pct / 100.0
                    + em * font_size
                    + vw * vp.width / 100.0
                    + vh * vp.height / 100.0,
            )
        }
        Some(val) => Some(val.to_px(16.0, vp.width, vp.height)),
    }
}

/// `text-transform: capitalize` — uppercase the first letter of each whitespace-
/// separated word. Correct now that text nodes are coalesced into runs.
/// Collapse runs of whitespace to a single space (CSS white-space: normal). Without
/// this, source-formatted HTML renders its inter-tag newlines and indentation.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                out.push(' ');
            }
            prev_ws = true;
        } else {
            out.push(ch);
            prev_ws = false;
        }
    }
    out
}

/// Extract the document `<title>` — the text the browser chrome shows as the tab /
/// window title. Walks the DOM for the first `<title>` element and returns its text,
/// whitespace-collapsed and trimmed. `None` if there is no `<title>` or it is empty.
/// Without this the browser has no page title to display.
pub fn document_title(dom: &DomNode) -> Option<String> {
    fn find_title(node: &DomNode) -> Option<&DomNode> {
        if let Some(tag) = node.tag_name() {
            if tag.eq_ignore_ascii_case("title") {
                return Some(node);
            }
        }
        for c in &node.children {
            if let Some(t) = find_title(c) {
                return Some(t);
            }
        }
        None
    }
    let title = find_title(dom)?;
    let collapsed = collapse_whitespace(&title.text_content());
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(String::from(trimmed))
    }
}

fn capitalize_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            at_word_start = true;
            out.push(ch);
        } else if at_word_start {
            for u in ch.to_uppercase() {
                out.push(u);
            }
            at_word_start = false;
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn build_layout_tree(styled: &StyledNode, vp: &Viewport) -> LayoutBox {
    let display = styled.style.display();
    if display == DisplayMode::None {
        return LayoutBox::new(DisplayMode::None);
    }

    let mut layout = LayoutBox::new(display);
    layout.position = styled.style.position();
    layout.tag_name = styled.node.tag_name().map(|s| s.to_string());
    layout.node_id = styled.node.element_id().map(|s| s.to_string());
    layout.style = styled.style.clone();

    if let NodeType::Text(ref text) = styled.node.node_type {
        // Apply `text-transform` (inherited from the containing element) to the
        // rendered text. Previously the property was parsed + inherited but never
        // applied, so `text-transform: uppercase` had no visible effect.
        // Collapse runs of whitespace (the newlines/indentation in source HTML) to a
        // single space per CSS white-space: normal; `pre`/`pre-wrap` preserve it.
        let preserve_ws = matches!(
            styled.style.get("white-space"),
            Some(CssValue::Keyword(k)) if k == "pre" || k == "pre-wrap"
        );
        let collapsed = if preserve_ws {
            text.clone()
        } else {
            collapse_whitespace(text)
        };
        let rendered = match styled.style.get("text-transform") {
            Some(CssValue::Keyword(k)) => match k.as_str() {
                "uppercase" => collapsed.to_uppercase(),
                "lowercase" => collapsed.to_lowercase(),
                "capitalize" => capitalize_words(&collapsed),
                _ => collapsed,
            },
            _ => collapsed,
        };
        layout.text = Some(rendered);
        layout.display = DisplayMode::Inline;
    }

    layout.z_index = match styled.style.get("z-index") {
        Some(CssValue::Number(n)) => *n as i32,
        _ => 0,
    };

    layout.opacity = match styled.style.get("opacity") {
        Some(CssValue::Number(n)) => n.clamp(0.0, 1.0),
        _ => 1.0,
    };

    layout.overflow_hidden = matches!(
        styled.style.get("overflow"),
        Some(CssValue::Keyword(k)) if k == "hidden"
    );

    layout.float = match styled.style.get("float") {
        Some(CssValue::Keyword(k)) if k == "left" => Some(FloatSide::Left),
        Some(CssValue::Keyword(k)) if k == "right" => Some(FloatSide::Right),
        _ => None,
    };

    let mut li_ordinal = 0usize;
    for child in &styled.children {
        let mut child_box = build_layout_tree(child, vp);
        if child_box.display == DisplayMode::ListItem {
            li_ordinal += 1;
            child_box.list_index = li_ordinal;
        }
        if child_box.display != DisplayMode::None {
            layout.children.push(child_box);
        }
    }

    layout
}

pub fn compute_layout(root: &mut LayoutBox, vp: &Viewport) {
    let containing = Rect {
        x: 0.0,
        y: 0.0,
        width: vp.width,
        height: vp.height,
    };
    layout_box(root, &containing, vp);
}

fn layout_box(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    match layout.display {
        DisplayMode::Block | DisplayMode::ListItem => layout_block(layout, containing, vp),
        DisplayMode::Inline | DisplayMode::InlineBlock => layout_inline(layout, containing, vp),
        DisplayMode::Flex => layout_flex(layout, containing, vp),
        DisplayMode::Grid => layout_grid(layout, containing, vp),
        DisplayMode::None => {}
    }
}

fn layout_block(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    layout.dimensions.margin = resolve_edge(&layout.style, "margin", vp);
    layout.dimensions.padding = resolve_edge(&layout.style, "padding", vp);
    layout.dimensions.border = resolve_edge(&layout.style, "border-width", vp);

    let margin_auto_lr = matches!(layout.style.get("margin"), Some(CssValue::Auto));

    let total_h = layout.dimensions.margin.horizontal()
        + layout.dimensions.padding.horizontal()
        + layout.dimensions.border.horizontal();

    // box-sizing: border-box (the modern reset default) means a specified width/
    // height INCLUDES padding + border, so the content box is smaller.
    let border_box = matches!(
        layout.style.get("box-sizing"),
        Some(CssValue::Keyword(k)) if k == "border-box"
    );

    let specified_w = resolve_dimension(&layout.style, "width", containing.width, vp);
    let width = specified_w.unwrap_or(containing.width - total_h);

    let min_w = resolve_dimension(&layout.style, "min-width", containing.width, vp);
    let max_w = resolve_dimension(&layout.style, "max-width", containing.width, vp);

    let mut w = width;
    if let Some(min) = min_w {
        w = w.max(min);
    }
    if let Some(max) = max_w {
        w = w.min(max);
    }
    // Only an explicit width subtracts box edges; `auto` already accounts for them.
    if border_box && specified_w.is_some() {
        w = (w - layout.dimensions.padding.horizontal() - layout.dimensions.border.horizontal())
            .max(0.0);
    }
    layout.dimensions.content.width = w;

    if margin_auto_lr {
        let remaining = containing.width - w - total_h;
        if remaining > 0.0 {
            layout.dimensions.margin.left = remaining / 2.0;
            layout.dimensions.margin.right = remaining / 2.0;
        }
    }

    layout.dimensions.content.x = containing.x
        + layout.dimensions.margin.left
        + layout.dimensions.border.left
        + layout.dimensions.padding.left;

    layout.dimensions.content.y = containing.y
        + layout.dimensions.margin.top
        + layout.dimensions.border.top
        + layout.dimensions.padding.top;

    let mut cursor_y = layout.dimensions.content.y;
    let mut prev_margin_bottom: f32 = 0.0;

    for child in &mut layout.children {
        let child_containing = Rect {
            x: layout.dimensions.content.x,
            y: cursor_y,
            width: layout.dimensions.content.width,
            height: 0.0,
        };
        layout_box(child, &child_containing, vp);

        let child_margin_top = child.dimensions.margin.top;
        let collapsed = prev_margin_bottom.max(child_margin_top);
        let skip = if prev_margin_bottom > 0.0 || child_margin_top > 0.0 {
            collapsed - child_margin_top
        } else {
            0.0
        };

        child.dimensions.content.y += skip;

        cursor_y = child.dimensions.margin_box().y + child.dimensions.margin_box().height;
        prev_margin_bottom = child.dimensions.margin.bottom;
    }

    let content_height = cursor_y - layout.dimensions.content.y;
    let specified_height = resolve_dimension(&layout.style, "height", containing.height, vp);
    let min_h = resolve_dimension(&layout.style, "min-height", containing.height, vp);
    let max_h = resolve_dimension(&layout.style, "max-height", containing.height, vp);

    let mut h = specified_height.unwrap_or(content_height);
    if let Some(min) = min_h {
        h = h.max(min);
    }
    if let Some(max) = max_h {
        h = h.min(max);
    }
    if border_box && specified_height.is_some() {
        h = (h - layout.dimensions.padding.vertical() - layout.dimensions.border.vertical())
            .max(0.0);
    }
    layout.dimensions.content.height = h;

    apply_position_offsets(layout, containing, vp);
}

fn layout_inline(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    layout.dimensions.margin = resolve_edge(&layout.style, "margin", vp);
    layout.dimensions.padding = resolve_edge(&layout.style, "padding", vp);
    layout.dimensions.border = resolve_edge(&layout.style, "border-width", vp);

    layout.dimensions.content.x = containing.x
        + layout.dimensions.margin.left
        + layout.dimensions.border.left
        + layout.dimensions.padding.left;
    layout.dimensions.content.y = containing.y
        + layout.dimensions.margin.top
        + layout.dimensions.border.top
        + layout.dimensions.padding.top;

    if let Some(ref text) = layout.text {
        let font_size = match layout.style.get("font-size") {
            Some(val) => val.to_px(16.0, vp.width, vp.height),
            None => 16.0,
        };
        let char_width = font_size * 0.6;
        let available = containing.width
            - layout.dimensions.padding.horizontal()
            - layout.dimensions.border.horizontal();

        let chars_per_line = if char_width > 0.0 {
            (available / char_width) as usize
        } else {
            80
        };
        let chars_per_line = chars_per_line.max(1);

        // `white-space: nowrap` / `pre` keep the text on a single line (it may
        // overflow) instead of breaking by width. `pre-wrap`/`pre-line` still wrap.
        let nowrap = matches!(
            layout.style.get("white-space"),
            Some(CssValue::Keyword(k)) if k == "nowrap" || k == "pre"
        );
        let line_count = if nowrap {
            1
        } else {
            (text.len() + chars_per_line - 1) / chars_per_line
        };
        let line_height = match layout.style.get("line-height") {
            Some(CssValue::Number(n)) => n * font_size,
            Some(val) => val.to_px(font_size, vp.width, vp.height),
            None => font_size * 1.2,
        };

        layout.dimensions.content.width = if line_count == 1 {
            text.len() as f32 * char_width
        } else {
            available
        };
        // text-align (inherited from the block parent) shifts a single line within
        // the available width. Multi-line wrapped text is left at the start edge.
        if line_count == 1 {
            let slack = (available - layout.dimensions.content.width).max(0.0);
            match layout.style.get("text-align") {
                Some(CssValue::Keyword(k)) if k == "center" => {
                    layout.dimensions.content.x += slack / 2.0;
                }
                Some(CssValue::Keyword(k)) if k == "right" || k == "end" => {
                    layout.dimensions.content.x += slack;
                }
                _ => {}
            }
        }
        layout.dimensions.content.height = line_count as f32 * line_height;
    } else {
        let specified_w = resolve_dimension(&layout.style, "width", containing.width, vp);
        let specified_h = resolve_dimension(&layout.style, "height", containing.height, vp);
        layout.dimensions.content.width = specified_w.unwrap_or(0.0);
        layout.dimensions.content.height = specified_h.unwrap_or(0.0);

        let mut cx = layout.dimensions.content.x;
        let mut max_h: f32 = 0.0;
        let mut line_y = layout.dimensions.content.y;

        for child in &mut layout.children {
            let child_containing = Rect {
                x: cx,
                y: line_y,
                width: containing.width - (cx - containing.x),
                height: containing.height,
            };
            layout_box(child, &child_containing, vp);

            let cw = child.dimensions.margin_box().width;
            let ch = child.dimensions.margin_box().height;

            if cx + cw > containing.x + containing.width && cx > containing.x {
                cx = containing.x;
                line_y += max_h;
                max_h = 0.0;

                child.dimensions.content.x = cx
                    + child.dimensions.margin.left
                    + child.dimensions.border.left
                    + child.dimensions.padding.left;
                child.dimensions.content.y = line_y
                    + child.dimensions.margin.top
                    + child.dimensions.border.top
                    + child.dimensions.padding.top;
            }

            cx += cw;
            max_h = max_h.max(ch);
        }
        // Keep an explicit width/height (CSS or <img width=200 height=100>); otherwise
        // fall back to the content-derived height / the full container width.
        let content_h = (line_y + max_h) - layout.dimensions.content.y;
        let mut w = specified_w.unwrap_or(containing.width);
        let mut h = specified_h.unwrap_or(content_h);
        // min/max-width and -height clamp inline-block boxes (e.g. `button { min-width }`).
        if let Some(min) = resolve_dimension(&layout.style, "min-width", containing.width, vp) {
            w = w.max(min);
        }
        if let Some(max) = resolve_dimension(&layout.style, "max-width", containing.width, vp) {
            w = w.min(max);
        }
        if let Some(min) = resolve_dimension(&layout.style, "min-height", containing.height, vp) {
            h = h.max(min);
        }
        if let Some(max) = resolve_dimension(&layout.style, "max-height", containing.height, vp) {
            h = h.min(max);
        }
        layout.dimensions.content.width = w;
        layout.dimensions.content.height = h;
    }

    apply_position_offsets(layout, containing, vp);
}

/// flex-grow factor for a flex item, from `flex-grow` or the first number of the
/// `flex` shorthand (`flex: 1` -> 1, `flex: 1 1 auto` -> 1). Defaults to 0.
fn flex_grow_of(style: &ComputedStyle) -> f32 {
    if let Some(CssValue::Number(n)) = style.get("flex-grow") {
        return *n;
    }
    match style.get("flex") {
        Some(CssValue::Number(n)) => *n,
        Some(CssValue::Multiple(items)) => items
            .iter()
            .find_map(|v| match v {
                CssValue::Number(n) => Some(*n),
                _ => None,
            })
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

fn layout_flex(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    layout.dimensions.margin = resolve_edge(&layout.style, "margin", vp);
    layout.dimensions.padding = resolve_edge(&layout.style, "padding", vp);
    layout.dimensions.border = resolve_edge(&layout.style, "border-width", vp);

    let total_h = layout.dimensions.margin.horizontal()
        + layout.dimensions.padding.horizontal()
        + layout.dimensions.border.horizontal();

    let width = resolve_dimension(&layout.style, "width", containing.width, vp)
        .unwrap_or(containing.width - total_h);
    layout.dimensions.content.width = width;

    layout.dimensions.content.x = containing.x
        + layout.dimensions.margin.left
        + layout.dimensions.border.left
        + layout.dimensions.padding.left;
    layout.dimensions.content.y = containing.y
        + layout.dimensions.margin.top
        + layout.dimensions.border.top
        + layout.dimensions.padding.top;

    let is_column = matches!(
        layout.style.get("flex-direction"),
        Some(CssValue::Keyword(k)) if k == "column" || k == "column-reverse"
    );
    let is_wrap = matches!(
        layout.style.get("flex-wrap"),
        Some(CssValue::Keyword(k)) if k == "wrap" || k == "wrap-reverse"
    );

    let gap = match layout.style.get("gap") {
        Some(val) => val.to_px(16.0, vp.width, vp.height),
        None => 0.0,
    };

    for child in &mut layout.children {
        let child_containing = Rect {
            x: layout.dimensions.content.x,
            y: layout.dimensions.content.y,
            width: layout.dimensions.content.width,
            height: containing.height,
        };
        layout_box(child, &child_containing, vp);
    }

    let child_count = layout.children.len();
    if child_count == 0 {
        layout.dimensions.content.height =
            resolve_dimension(&layout.style, "height", containing.height, vp).unwrap_or(0.0);
        return;
    }

    let total_gap = if child_count > 1 {
        gap * (child_count - 1) as f32
    } else {
        0.0
    };

    if is_column {
        let total_child_h: f32 = layout
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().height)
            .sum();

        let available = resolve_dimension(&layout.style, "height", containing.height, vp)
            .unwrap_or(total_child_h + total_gap);
        let remaining = available - total_child_h - total_gap;

        let _total_grow: f32 = layout
            .children
            .iter()
            .enumerate()
            .map(|(_, _)| 1.0_f32)
            .sum();

        let mut cy = layout.dimensions.content.y;
        let justify = layout
            .style
            .get("justify-content")
            .and_then(|v| {
                if let CssValue::Keyword(k) = v {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| String::from("flex-start"));

        let spacing = match justify.as_str() {
            "center" => {
                cy += remaining / 2.0;
                gap
            }
            "flex-end" => {
                cy += remaining;
                gap
            }
            "space-between" if child_count > 1 => {
                (remaining + total_gap) / (child_count - 1) as f32
            }
            "space-around" if child_count > 0 => {
                let s = (remaining + total_gap) / child_count as f32;
                cy += s / 2.0;
                s
            }
            "space-evenly" if child_count > 0 => {
                let s = (remaining + total_gap) / (child_count + 1) as f32;
                cy += s;
                s
            }
            _ => gap,
        };

        for child in &mut layout.children {
            child.dimensions.content.y = cy
                + child.dimensions.margin.top
                + child.dimensions.border.top
                + child.dimensions.padding.top;
            cy += child.dimensions.margin_box().height + spacing;
        }

        layout.dimensions.content.height = cy - layout.dimensions.content.y;
    } else if is_wrap {
        // flex-wrap: wrap — a ROW container whose items don't fit the main-axis
        // width break onto additional lines (responsive card grids, tag lists,
        // nav bars). Isolated from the single-line path below so non-wrapping
        // flex is untouched. Column-axis wrap is not handled (rare).
        layout_flex_row_wrap(layout, width, gap, vp, containing);
    } else {
        let total_child_w: f32 = layout
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().width)
            .sum();

        let remaining = width - total_child_w - total_gap;

        // flex-grow: distribute leftover main-axis space across items by their grow
        // factor (`flex: 1`). Grown items consume the slack, so justify-content then
        // has nothing extra to spread (remaining -> 0).
        let grows: Vec<f32> = layout
            .children
            .iter()
            .map(|c| flex_grow_of(&c.style))
            .collect();
        let total_grow: f32 = grows.iter().sum();
        let remaining = if total_grow > 0.0 && remaining > 0.0 {
            for (child, &g) in layout.children.iter_mut().zip(grows.iter()) {
                if g > 0.0 {
                    child.dimensions.content.width += remaining * g / total_grow;
                }
            }
            0.0
        } else {
            remaining
        };

        let justify = layout
            .style
            .get("justify-content")
            .and_then(|v| {
                if let CssValue::Keyword(k) = v {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| String::from("flex-start"));

        let mut cx = layout.dimensions.content.x;

        let spacing = match justify.as_str() {
            "center" => {
                cx += remaining / 2.0;
                gap
            }
            "flex-end" => {
                cx += remaining;
                gap
            }
            "space-between" if child_count > 1 => {
                (remaining + total_gap) / (child_count - 1) as f32
            }
            "space-around" if child_count > 0 => {
                let s = (remaining + total_gap) / child_count as f32;
                cx += s / 2.0;
                s
            }
            "space-evenly" if child_count > 0 => {
                let s = (remaining + total_gap) / (child_count + 1) as f32;
                cx += s;
                s
            }
            _ => gap,
        };

        let mut max_child_h: f32 = 0.0;
        for child in &mut layout.children {
            child.dimensions.content.x = cx
                + child.dimensions.margin.left
                + child.dimensions.border.left
                + child.dimensions.padding.left;
            cx += child.dimensions.margin_box().width + spacing;
            max_child_h = max_child_h.max(child.dimensions.margin_box().height);
        }

        let align = layout
            .style
            .get("align-items")
            .and_then(|v| {
                if let CssValue::Keyword(k) = v {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| String::from("stretch"));

        for child in &mut layout.children {
            let child_h = child.dimensions.margin_box().height;
            match align.as_str() {
                "center" => {
                    let offset = (max_child_h - child_h) / 2.0;
                    child.dimensions.content.y += offset;
                }
                "flex-end" => {
                    let offset = max_child_h - child_h;
                    child.dimensions.content.y += offset;
                }
                "stretch" => {
                    child.dimensions.content.height = max_child_h
                        - child.dimensions.margin.vertical()
                        - child.dimensions.border.vertical()
                        - child.dimensions.padding.vertical();
                }
                _ => {}
            }
        }

        layout.dimensions.content.height =
            resolve_dimension(&layout.style, "height", containing.height, vp)
                .unwrap_or(max_child_h);
    }

    apply_position_offsets(layout, containing, vp);
}

/// `flex-wrap: wrap` for a ROW flex container: pack children into lines that fit
/// the content `width`, distributing per-line flex-grow slack, stacking lines
/// down the cross axis (separated by `gap`), and stretching each item to its
/// line's height. Children are already laid out; we only (re)position them.
fn layout_flex_row_wrap(
    layout: &mut LayoutBox,
    width: f32,
    gap: f32,
    vp: &Viewport,
    containing: &Rect,
) {
    // 1. Break children (by index) into lines by main-axis fit. An item always
    // starts its own line if it alone exceeds the width (no infinite loop).
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut line_w = 0.0f32;
    for (idx, child) in layout.children.iter().enumerate() {
        let cw = child.dimensions.margin_box().width;
        let with_item = if cur.is_empty() {
            cw
        } else {
            line_w + gap + cw
        };
        if !cur.is_empty() && with_item > width {
            lines.push(core::mem::take(&mut cur));
            cur.push(idx);
            line_w = cw;
        } else {
            cur.push(idx);
            line_w = with_item;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }

    let origin_x = layout.dimensions.content.x;
    let mut cy = layout.dimensions.content.y;
    let mut total_h = 0.0f32;
    let justify = layout
        .style
        .get("justify-content")
        .and_then(|v| {
            if let CssValue::Keyword(k) = v {
                Some(k.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| String::from("flex-start"));

    for (li, line) in lines.iter().enumerate() {
        if li > 0 {
            cy += gap; // cross-axis gap between lines
            total_h += gap;
        }
        // Per-line flex-grow: distribute the line's leftover main-axis space.
        let line_child_w: f32 = line
            .iter()
            .map(|&i| layout.children[i].dimensions.margin_box().width)
            .sum();
        let line_gap = gap * line.len().saturating_sub(1) as f32;
        let remaining = width - line_child_w - line_gap;
        let grows: Vec<f32> = line
            .iter()
            .map(|&i| flex_grow_of(&layout.children[i].style))
            .collect();
        let total_grow: f32 = grows.iter().sum();
        // Slack the line still has after flex-grow (grow consumes it all).
        let line_remaining = if total_grow > 0.0 && remaining > 0.0 {
            for (&i, &g) in line.iter().zip(grows.iter()) {
                if g > 0.0 {
                    layout.children[i].dimensions.content.width += remaining * g / total_grow;
                }
            }
            0.0
        } else {
            remaining.max(0.0)
        };
        // The line's cross size is the tallest item's margin-box height.
        let line_h: f32 = line
            .iter()
            .map(|&i| layout.children[i].dimensions.margin_box().height)
            .fold(0.0, f32::max);
        // justify-content distributes the line's leftover main-axis slack (same
        // semantics as the single-line path, applied per wrapped line).
        let n = line.len();
        let mut item_spacing = gap;
        let mut start_x = origin_x;
        match justify.as_str() {
            "center" => start_x += line_remaining / 2.0,
            "flex-end" | "end" => start_x += line_remaining,
            "space-between" if n > 1 => item_spacing = gap + line_remaining / (n - 1) as f32,
            "space-around" if n > 0 => {
                let s = line_remaining / n as f32;
                start_x += s / 2.0;
                item_spacing = gap + s;
            }
            "space-evenly" if n > 0 => {
                let s = line_remaining / (n + 1) as f32;
                start_x += s;
                item_spacing = gap + s;
            }
            _ => {}
        }
        // Position items along the main axis and pin them to the line's
        // cross-axis top, stretching each to the line height.
        let mut cx = start_x;
        for &i in line {
            let child = &mut layout.children[i];
            child.dimensions.content.x = cx
                + child.dimensions.margin.left
                + child.dimensions.border.left
                + child.dimensions.padding.left;
            child.dimensions.content.y = cy
                + child.dimensions.margin.top
                + child.dimensions.border.top
                + child.dimensions.padding.top;
            child.dimensions.content.height = line_h
                - child.dimensions.margin.vertical()
                - child.dimensions.border.vertical()
                - child.dimensions.padding.vertical();
            cx += child.dimensions.margin_box().width + item_spacing;
        }
        cy += line_h;
        total_h += line_h;
    }

    layout.dimensions.content.height =
        resolve_dimension(&layout.style, "height", containing.height, vp).unwrap_or(total_h);
}

fn layout_grid(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    layout.dimensions.margin = resolve_edge(&layout.style, "margin", vp);
    layout.dimensions.padding = resolve_edge(&layout.style, "padding", vp);
    layout.dimensions.border = resolve_edge(&layout.style, "border-width", vp);

    let total_h = layout.dimensions.margin.horizontal()
        + layout.dimensions.padding.horizontal()
        + layout.dimensions.border.horizontal();

    let width = resolve_dimension(&layout.style, "width", containing.width, vp)
        .unwrap_or(containing.width - total_h);
    layout.dimensions.content.width = width;
    layout.dimensions.content.x = containing.x
        + layout.dimensions.margin.left
        + layout.dimensions.border.left
        + layout.dimensions.padding.left;
    layout.dimensions.content.y = containing.y
        + layout.dimensions.margin.top
        + layout.dimensions.border.top
        + layout.dimensions.padding.top;

    let gap = match layout.style.get("gap") {
        Some(val) => val.to_px(16.0, vp.width, vp.height),
        None => 0.0,
    };

    let col_count = match layout.style.get("grid-template-columns") {
        Some(CssValue::Multiple(vals)) => vals.len().max(1),
        _ => 1,
    };

    let col_width = (width - gap * (col_count as f32 - 1.0).max(0.0)) / col_count as f32;

    let mut col = 0;
    let mut row_y = layout.dimensions.content.y;
    let mut row_height: f32 = 0.0;

    for child in &mut layout.children {
        let cx = layout.dimensions.content.x + (col as f32) * (col_width + gap);
        let child_containing = Rect {
            x: cx,
            y: row_y,
            width: col_width,
            height: containing.height,
        };
        layout_box(child, &child_containing, vp);
        child.dimensions.content.x = cx
            + child.dimensions.margin.left
            + child.dimensions.border.left
            + child.dimensions.padding.left;
        child.dimensions.content.y = row_y
            + child.dimensions.margin.top
            + child.dimensions.border.top
            + child.dimensions.padding.top;

        row_height = row_height.max(child.dimensions.margin_box().height);
        col += 1;
        if col >= col_count {
            col = 0;
            row_y += row_height + gap;
            row_height = 0.0;
        }
    }
    if col > 0 {
        row_y += row_height;
    }

    layout.dimensions.content.height =
        resolve_dimension(&layout.style, "height", containing.height, vp)
            .unwrap_or(row_y - layout.dimensions.content.y);

    apply_position_offsets(layout, containing, vp);
}

fn apply_position_offsets(layout: &mut LayoutBox, containing: &Rect, vp: &Viewport) {
    match layout.position {
        Position::Relative => {
            if let Some(top) = layout.style.get("top") {
                layout.dimensions.content.y += top.to_px(16.0, vp.width, vp.height);
            }
            if let Some(left) = layout.style.get("left") {
                layout.dimensions.content.x += left.to_px(16.0, vp.width, vp.height);
            }
        }
        Position::Absolute => {
            if let Some(top) = layout.style.get("top") {
                layout.dimensions.content.y = containing.y + top.to_px(16.0, vp.width, vp.height);
            }
            if let Some(left) = layout.style.get("left") {
                layout.dimensions.content.x = containing.x + left.to_px(16.0, vp.width, vp.height);
            }
            if let Some(right) = layout.style.get("right") {
                let r = right.to_px(16.0, vp.width, vp.height);
                layout.dimensions.content.x =
                    containing.x + containing.width - layout.dimensions.content.width - r;
            }
            if let Some(bottom) = layout.style.get("bottom") {
                let b = bottom.to_px(16.0, vp.width, vp.height);
                layout.dimensions.content.y =
                    containing.y + containing.height - layout.dimensions.content.height - b;
            }
        }
        Position::Fixed => {
            if let Some(top) = layout.style.get("top") {
                layout.dimensions.content.y = top.to_px(16.0, vp.width, vp.height);
            }
            if let Some(left) = layout.style.get("left") {
                layout.dimensions.content.x = left.to_px(16.0, vp.width, vp.height);
            }
            if let Some(right) = layout.style.get("right") {
                let r = right.to_px(16.0, vp.width, vp.height);
                layout.dimensions.content.x = vp.width - layout.dimensions.content.width - r;
            }
            if let Some(bottom) = layout.style.get("bottom") {
                let b = bottom.to_px(16.0, vp.width, vp.height);
                layout.dimensions.content.y = vp.height - layout.dimensions.content.height - b;
            }
        }
        Position::Sticky | Position::Static => {}
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  6.  RENDERING PIPELINE
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum PaintCommand {
    FillRect {
        rect: Rect,
        color: CssColor,
    },
    /// A vertical (top->bottom) gradient fill for `linear-gradient` backgrounds.
    FillGradient {
        rect: Rect,
        top: CssColor,
        bottom: CssColor,
    },
    StrokeRect {
        rect: Rect,
        color: CssColor,
        width: f32,
    },
    FillText {
        text: String,
        x: f32,
        y: f32,
        font_size: f32,
        color: CssColor,
        font_family: String,
        font_weight: String,
        underline: bool,
        strikethrough: bool,
    },
    DrawImage {
        src: String,
        rect: Rect,
    },
    FillRoundedRect {
        rect: Rect,
        color: CssColor,
        radius: f32,
    },
    SetClip {
        rect: Rect,
    },
    ClearClip,
    PushOpacity(f32),
    PopOpacity,
    DrawBorder {
        rect: Rect,
        color: CssColor,
        widths: EdgeSizes,
        radius: f32,
    },
    DrawShadow {
        rect: Rect,
        color: CssColor,
        offset_x: f32,
        offset_y: f32,
        blur: f32,
        spread: f32,
    },
}

#[derive(Debug)]
pub struct DisplayList {
    pub commands: Vec<PaintCommand>,
}

impl DisplayList {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn push(&mut self, cmd: PaintCommand) {
        self.commands.push(cmd);
    }
}

pub fn build_display_list(layout: &LayoutBox, vp: &Viewport) -> DisplayList {
    let mut list = DisplayList::new();
    paint_layout_box(layout, &mut list, vp);
    list
}

fn paint_layout_box(layout: &LayoutBox, list: &mut DisplayList, vp: &Viewport) {
    if layout.display == DisplayMode::None {
        return;
    }

    let push_opacity = layout.opacity < 1.0;
    if push_opacity {
        list.push(PaintCommand::PushOpacity(layout.opacity));
    }

    if layout.overflow_hidden {
        list.push(PaintCommand::SetClip {
            rect: layout.dimensions.border_box(),
        });
    }

    // visibility: hidden (or collapse) keeps the box's layout space but paints
    // nothing of its OWN. Children still recurse — visibility is inherited, so they
    // are hidden too unless a descendant sets visibility: visible.
    let visible = !matches!(
        layout.style.get("visibility"),
        Some(CssValue::Keyword(k)) if k == "hidden" || k == "collapse"
    );
    if visible {
        paint_box_shadow(layout, list);
        paint_background(layout, list);
        paint_borders(layout, list);
        paint_text(layout, list);
        paint_list_marker(layout, list);
    }

    let mut children_with_z: Vec<(i32, usize)> = layout
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| (c.z_index, i))
        .collect();
    children_with_z.sort_by_key(|&(z, i)| (z, i));

    for &(_, idx) in &children_with_z {
        paint_layout_box(&layout.children[idx], list, vp);
    }

    if layout.overflow_hidden {
        list.push(PaintCommand::ClearClip);
    }

    if push_opacity {
        list.push(PaintCommand::PopOpacity);
    }
}

/// Extract the (top, bottom) colors of a VERTICAL `linear-gradient` (the CSS
/// default direction). Returns None for `to left`/`to right` or an angle (parsed as
/// a Length) since raegfx's gradient fill is row-interpolated (vertical) only.
fn parse_vertical_gradient(args: &[CssValue]) -> Option<(CssColor, CssColor)> {
    let has_horizontal = args.iter().any(|v| {
        matches!(v, CssValue::Keyword(k) if k == "left" || k == "right")
            || matches!(v, CssValue::Length(..))
    });
    if has_horizontal {
        return None;
    }
    let colors: alloc::vec::Vec<CssColor> = args
        .iter()
        .filter_map(|v| match v {
            CssValue::Color(c) => Some(*c),
            _ => None,
        })
        .collect();
    if colors.len() < 2 {
        return None;
    }
    let to_top = args.windows(2).any(|w| {
        matches!((&w[0], &w[1]),
            (CssValue::Keyword(a), CssValue::Keyword(b)) if a == "to" && b == "top")
    });
    let first = colors[0];
    let last = colors[colors.len() - 1];
    if to_top {
        Some((last, first))
    } else {
        Some((first, last))
    }
}

fn paint_background(layout: &LayoutBox, list: &mut DisplayList) {
    // linear-gradient background: vertical maps to the gradient fill; an unsupported
    // direction degrades to the first color as a solid fill.
    if let Some(CssValue::Function(name, args)) = layout
        .style
        .get("background-image")
        .or(layout.style.get("background"))
    {
        if name == "linear-gradient" || name == "repeating-linear-gradient" {
            if let Some((top, bottom)) = parse_vertical_gradient(args) {
                list.push(PaintCommand::FillGradient {
                    rect: layout.dimensions.border_box(),
                    top,
                    bottom,
                });
                return;
            }
            if let Some(CssValue::Color(c)) = args.iter().find(|v| matches!(v, CssValue::Color(_)))
            {
                list.push(PaintCommand::FillRect {
                    rect: layout.dimensions.border_box(),
                    color: *c,
                });
            }
            return;
        }
    }

    let bg_color = match layout
        .style
        .get("background-color")
        .or(layout.style.get("background"))
    {
        Some(CssValue::Color(c)) => *c,
        // `background` shorthand with more than a color (`#fff url(x) no-repeat`)
        // parses to a Multiple -- pull out its color component.
        Some(CssValue::Multiple(items)) => {
            match items.iter().find_map(|v| match v {
                CssValue::Color(c) => Some(*c),
                _ => None,
            }) {
                Some(c) => c,
                None => return,
            }
        }
        _ => return,
    };

    let border_radius = match layout.style.get("border-radius") {
        Some(val) => val.to_px(16.0, 0.0, 0.0),
        None => 0.0,
    };

    let rect = layout.dimensions.border_box();
    if border_radius > 0.0 {
        list.push(PaintCommand::FillRoundedRect {
            rect,
            color: bg_color,
            radius: border_radius,
        });
    } else {
        list.push(PaintCommand::FillRect {
            rect,
            color: bg_color,
        });
    }
}

/// The element's `color` (used to resolve `currentColor` / the default border color);
/// black if `color` is unset.
fn current_color(style: &ComputedStyle) -> CssColor {
    match style.get("color") {
        Some(CssValue::Color(c)) => *c,
        _ => CssColor::BLACK,
    }
}

fn paint_borders(layout: &LayoutBox, list: &mut DisplayList) {
    let bw = &layout.dimensions.border;
    if bw.top == 0.0 && bw.right == 0.0 && bw.bottom == 0.0 && bw.left == 0.0 {
        return;
    }

    // CSS default border-color is `currentColor` (the element's `color`), NOT black;
    // the `currentColor` keyword resolves the same way.
    let border_color = match layout.style.get("border-color") {
        Some(CssValue::Color(c)) => *c,
        _ => current_color(&layout.style),
    };

    let border_radius = match layout.style.get("border-radius") {
        Some(val) => val.to_px(16.0, 0.0, 0.0),
        None => 0.0,
    };

    list.push(PaintCommand::DrawBorder {
        rect: layout.dimensions.border_box(),
        color: border_color,
        widths: *bw,
        radius: border_radius,
    });
}

fn paint_text(layout: &LayoutBox, list: &mut DisplayList) {
    let text = match &layout.text {
        Some(t) if !t.is_empty() => t.clone(),
        _ => return,
    };

    let color = match layout.style.get("color") {
        Some(CssValue::Color(c)) => *c,
        _ => CssColor::BLACK,
    };

    let font_size = match layout.style.get("font-size") {
        Some(val) => val.to_px(16.0, 0.0, 0.0),
        None => 16.0,
    };

    let font_family = match layout.style.get("font-family") {
        Some(CssValue::Keyword(f)) => f.clone(),
        Some(CssValue::Raw(f)) => f.clone(),
        _ => String::from("sans-serif"),
    };

    let font_weight = match layout.style.get("font-weight") {
        Some(CssValue::Keyword(w)) => w.clone(),
        Some(CssValue::Number(n)) => {
            if *n >= 700.0 {
                String::from("bold")
            } else {
                String::from("normal")
            }
        }
        _ => String::from("normal"),
    };

    // text-decoration: underline (set by default on <a>/<u>) -> a baseline rule.
    let underline = match layout.style.get("text-decoration") {
        Some(CssValue::Keyword(k)) => k == "underline",
        Some(CssValue::Multiple(items)) => items
            .iter()
            .any(|v| matches!(v, CssValue::Keyword(k) if k == "underline")),
        _ => false,
    };
    let strikethrough = match layout.style.get("text-decoration") {
        Some(CssValue::Keyword(k)) => k == "line-through",
        Some(CssValue::Multiple(items)) => items
            .iter()
            .any(|v| matches!(v, CssValue::Keyword(k) if k == "line-through")),
        _ => false,
    };

    // text-shadow: <ox> <oy> [blur] [color] -> an offset copy drawn behind the text.
    // Hard shadow (blur approximated as none, like many simple engines).
    if let Some(CssValue::Multiple(parts)) = layout.style.get("text-shadow") {
        let lens: alloc::vec::Vec<f32> = parts
            .iter()
            .filter_map(|v| match v {
                CssValue::Length(n, _) | CssValue::Number(n) => Some(*n),
                _ => None,
            })
            .collect();
        if lens.len() >= 2 {
            let shadow_color = parts
                .iter()
                .find_map(|v| match v {
                    CssValue::Color(c) => Some(*c),
                    _ => None,
                })
                .unwrap_or(CssColor::rgba(0, 0, 0, 0.5));
            list.push(PaintCommand::FillText {
                text: text.clone(),
                x: layout.dimensions.content.x + lens[0],
                y: layout.dimensions.content.y + lens[1],
                font_size,
                color: shadow_color,
                font_family: font_family.clone(),
                font_weight: font_weight.clone(),
                underline: false,
                strikethrough: false,
            });
        }
    }

    list.push(PaintCommand::FillText {
        text,
        x: layout.dimensions.content.x,
        y: layout.dimensions.content.y,
        font_size,
        color,
        font_family,
        font_weight,
        underline,
        strikethrough,
    });
}

/// Draw the marker (bullet or ordinal) for a `display: list-item` box, to the left
/// of its content within the list's indent. `list-style-type: none` suppresses it.
fn paint_list_marker(layout: &LayoutBox, list: &mut DisplayList) {
    if layout.display != DisplayMode::ListItem {
        return;
    }
    let lst = match layout.style.get("list-style-type") {
        Some(CssValue::Keyword(k)) => k.as_str(),
        // `list-style-type: none` parses to CssValue::None, not a keyword.
        Some(CssValue::None) => return,
        _ => "disc",
    };
    let marker = match lst {
        "decimal" => alloc::format!("{}.", layout.list_index),
        "circle" => String::from("\u{25E6}"),
        "square" => String::from("\u{25AA}"),
        _ => String::from("\u{2022}"),
    };
    let font_size = match layout.style.get("font-size") {
        Some(val) => val.to_px(16.0, 0.0, 0.0),
        None => 16.0,
    };
    let color = match layout.style.get("color") {
        Some(CssValue::Color(c)) => *c,
        _ => CssColor::BLACK,
    };
    list.push(PaintCommand::FillText {
        text: marker,
        x: layout.dimensions.content.x - font_size * 1.2,
        y: layout.dimensions.content.y,
        font_size,
        color,
        font_family: String::from("sans-serif"),
        font_weight: String::from("normal"),
        underline: false,
        strikethrough: false,
    });
}

fn paint_box_shadow(layout: &LayoutBox, list: &mut DisplayList) {
    if let Some(CssValue::Multiple(parts)) = layout.style.get("box-shadow") {
        let nums: Vec<f32> = parts
            .iter()
            .filter_map(|v| match v {
                CssValue::Number(n) | CssValue::Length(n, _) => Some(*n),
                _ => None,
            })
            .collect();
        let color = parts
            .iter()
            .find_map(|v| match v {
                CssValue::Color(c) => Some(*c),
                _ => None,
            })
            .unwrap_or(CssColor::rgba(0, 0, 0, 0.25));

        if nums.len() >= 2 {
            list.push(PaintCommand::DrawShadow {
                rect: layout.dimensions.border_box(),
                color,
                offset_x: nums[0],
                offset_y: nums[1],
                blur: nums.get(2).copied().unwrap_or(0.0),
                spread: nums.get(3).copied().unwrap_or(0.0),
            });
        }
    }
}

// ── Hit Testing ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HitTestResult {
    pub tag_name: Option<String>,
    pub node_id: Option<String>,
    pub rect: Rect,
}

pub fn hit_test(layout: &LayoutBox, x: f32, y: f32) -> Option<HitTestResult> {
    for child in layout.children.iter().rev() {
        if let Some(result) = hit_test(child, x, y) {
            return Some(result);
        }
    }

    let border_box = layout.dimensions.border_box();
    if border_box.contains_point(x, y) && layout.display != DisplayMode::None {
        return Some(HitTestResult {
            tag_name: layout.tag_name.clone(),
            node_id: layout.node_id.clone(),
            rect: border_box,
        });
    }

    None
}

pub fn hit_test_all(layout: &LayoutBox, x: f32, y: f32) -> Vec<HitTestResult> {
    let mut results = Vec::new();
    hit_test_recursive(layout, x, y, &mut results);
    results
}

fn hit_test_recursive(layout: &LayoutBox, x: f32, y: f32, results: &mut Vec<HitTestResult>) {
    let border_box = layout.dimensions.border_box();
    if border_box.contains_point(x, y) && layout.display != DisplayMode::None {
        results.push(HitTestResult {
            tag_name: layout.tag_name.clone(),
            node_id: layout.node_id.clone(),
            rect: border_box,
        });
    }
    for child in &layout.children {
        hit_test_recursive(child, x, y, results);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  PUBLIC PIPELINE
// ═══════════════════════════════════════════════════════════════════════════

pub struct RenderPipeline {
    pub viewport: Viewport,
}

impl RenderPipeline {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            viewport: Viewport::new(width, height),
        }
    }

    pub fn render(&self, html: &str, css: &str) -> DisplayList {
        let dom = parse_html(html);
        let stylesheet = expand_media_rules(&parse_css(css), self.viewport.width);
        let mut order = 0;
        let styled = build_styled_tree(&dom, &stylesheet, None, &mut order);
        let mut layout = build_layout_tree(&styled, &self.viewport);
        compute_layout(&mut layout, &self.viewport);
        build_display_list(&layout, &self.viewport)
    }

    pub fn render_to_layout(&self, html: &str, css: &str) -> LayoutBox {
        let dom = parse_html(html);
        let stylesheet = expand_media_rules(&parse_css(css), self.viewport.width);
        let mut order = 0;
        let styled = build_styled_tree(&dom, &stylesheet, None, &mut order);
        let mut layout = build_layout_tree(&styled, &self.viewport);
        compute_layout(&mut layout, &self.viewport);
        layout
    }

    /// The document `<title>` for the browser chrome (tab / window title). Parses and
    /// extracts in one call; `None` if the page has no non-empty `<title>`.
    pub fn page_title(&self, html: &str) -> Option<String> {
        document_title(&parse_html(html))
    }

    pub fn hit_test(&self, layout: &LayoutBox, x: f32, y: f32) -> Option<HitTestResult> {
        hit_test(layout, x, y)
    }

    pub fn scroll(&mut self, dx: f32, dy: f32) {
        self.viewport.scroll_x += dx;
        self.viewport.scroll_y += dy;
        if self.viewport.scroll_x < 0.0 {
            self.viewport.scroll_x = 0.0;
        }
        if self.viewport.scroll_y < 0.0 {
            self.viewport.scroll_y = 0.0;
        }
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.viewport.width = width;
        self.viewport.height = height;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  PWA RUNTIME — Web App Manifest, Install/Run, Service Worker, Notifications
// ═══════════════════════════════════════════════════════════════════════════

// ── PWA Manifest ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PwaDisplayMode {
    Fullscreen,
    Standalone,
    MinimalUi,
    Browser,
}

impl PwaDisplayMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "fullscreen" => Self::Fullscreen,
            "standalone" => Self::Standalone,
            "minimal-ui" => Self::MinimalUi,
            _ => Self::Browser,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PwaIcon {
    pub src: String,
    pub sizes: String,
    pub icon_type: String,
    pub purpose: String,
}

#[derive(Debug, Clone)]
pub struct PwaManifest {
    pub name: String,
    pub short_name: String,
    pub start_url: String,
    pub scope: String,
    pub display: PwaDisplayMode,
    pub background_color: CssColor,
    pub theme_color: CssColor,
    pub icons: Vec<PwaIcon>,
    pub description: String,
    pub lang: String,
    pub orientation: String,
}

impl PwaManifest {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            short_name: String::new(),
            start_url: String::from("/"),
            scope: String::from("/"),
            display: PwaDisplayMode::Standalone,
            background_color: CssColor::WHITE,
            theme_color: CssColor::WHITE,
            icons: Vec::new(),
            description: String::new(),
            lang: String::from("en"),
            orientation: String::from("any"),
        }
    }

    /// Minimal JSON manifest parser (no serde needed).
    /// Handles the subset of fields PWAs actually use.
    pub fn parse(json: &str) -> Self {
        let mut manifest = Self::new();
        let chars: Vec<char> = json.chars().collect();
        let len = chars.len();
        let mut i = 0;

        while i < len {
            if let Some(key_start) = find_next_string_start(&chars, i) {
                let (key, key_end) = extract_json_string(&chars, key_start);
                i = key_end;
                i = skip_to_char(&chars, i, ':') + 1;
                i = skip_whitespace_chars(&chars, i);

                if i >= len {
                    break;
                }

                match key.as_str() {
                    "name" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.name = s.0;
                            i = s.1;
                        }
                    }
                    "short_name" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.short_name = s.0;
                            i = s.1;
                        }
                    }
                    "start_url" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.start_url = s.0;
                            i = s.1;
                        }
                    }
                    "scope" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.scope = s.0;
                            i = s.1;
                        }
                    }
                    "display" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.display = PwaDisplayMode::from_str(&s.0);
                            i = s.1;
                        }
                    }
                    "background_color" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.background_color = parse_css_color_value(&s.0);
                            i = s.1;
                        }
                    }
                    "theme_color" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.theme_color = parse_css_color_value(&s.0);
                            i = s.1;
                        }
                    }
                    "description" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.description = s.0;
                            i = s.1;
                        }
                    }
                    "lang" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.lang = s.0;
                            i = s.1;
                        }
                    }
                    "orientation" => {
                        if let Some(s) = try_extract_string_value(&chars, i) {
                            manifest.orientation = s.0;
                            i = s.1;
                        }
                    }
                    "icons" => {
                        let (icons, end) = parse_icons_array(&chars, i);
                        manifest.icons = icons;
                        i = end;
                    }
                    _ => {
                        i = skip_json_value(&chars, i);
                    }
                }
            } else {
                break;
            }
        }

        if manifest.short_name.is_empty() {
            manifest.short_name = manifest.name.clone();
        }
        manifest
    }
}

fn find_next_string_start(chars: &[char], from: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn extract_json_string(chars: &[char], start: usize) -> (String, usize) {
    let mut s = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                '"' => {
                    s.push('"');
                    i += 2;
                }
                '\\' => {
                    s.push('\\');
                    i += 2;
                }
                'n' => {
                    s.push('\n');
                    i += 2;
                }
                't' => {
                    s.push('\t');
                    i += 2;
                }
                '/' => {
                    s.push('/');
                    i += 2;
                }
                _ => {
                    s.push(chars[i + 1]);
                    i += 2;
                }
            }
        } else if chars[i] == '"' {
            return (s, i + 1);
        } else {
            s.push(chars[i]);
            i += 1;
        }
    }
    (s, i)
}

fn try_extract_string_value(chars: &[char], from: usize) -> Option<(String, usize)> {
    if from < chars.len() && chars[from] == '"' {
        let (s, end) = extract_json_string(chars, from);
        Some((s, end))
    } else {
        None
    }
}

fn skip_to_char(chars: &[char], from: usize, target: char) -> usize {
    let mut i = from;
    while i < chars.len() && chars[i] != target {
        i += 1;
    }
    i
}

fn skip_whitespace_chars(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while i < chars.len()
        && (chars[i] == ' ' || chars[i] == '\t' || chars[i] == '\n' || chars[i] == '\r')
    {
        i += 1;
    }
    i
}

fn skip_json_value(chars: &[char], from: usize) -> usize {
    let i = skip_whitespace_chars(chars, from);
    if i >= chars.len() {
        return i;
    }
    match chars[i] {
        '"' => {
            let (_, end) = extract_json_string(chars, i);
            end
        }
        '{' | '[' => {
            let open = chars[i];
            let close = if open == '{' { '}' } else { ']' };
            let mut depth = 1;
            let mut j = i + 1;
            while j < chars.len() && depth > 0 {
                if chars[j] == open {
                    depth += 1;
                } else if chars[j] == close {
                    depth -= 1;
                } else if chars[j] == '"' {
                    let (_, end) = extract_json_string(chars, j);
                    j = end;
                    continue;
                }
                j += 1;
            }
            j
        }
        _ => {
            let mut j = i;
            while j < chars.len() && chars[j] != ',' && chars[j] != '}' && chars[j] != ']' {
                j += 1;
            }
            j
        }
    }
}

fn parse_icons_array(chars: &[char], from: usize) -> (Vec<PwaIcon>, usize) {
    let mut icons = Vec::new();
    let i = skip_whitespace_chars(chars, from);
    if i >= chars.len() || chars[i] != '[' {
        return (icons, i);
    }
    let mut j = i + 1;
    while j < chars.len() && chars[j] != ']' {
        j = skip_whitespace_chars(chars, j);
        if j >= chars.len() || chars[j] == ']' {
            break;
        }
        if chars[j] == '{' {
            let (icon, end) = parse_icon_object(chars, j);
            icons.push(icon);
            j = end;
        } else {
            j += 1;
        }
        j = skip_whitespace_chars(chars, j);
        if j < chars.len() && chars[j] == ',' {
            j += 1;
        }
    }
    if j < chars.len() && chars[j] == ']' {
        j += 1;
    }
    (icons, j)
}

fn parse_icon_object(chars: &[char], from: usize) -> (PwaIcon, usize) {
    let mut icon = PwaIcon {
        src: String::new(),
        sizes: String::new(),
        icon_type: String::new(),
        purpose: String::from("any"),
    };
    let mut j = from + 1;
    while j < chars.len() && chars[j] != '}' {
        j = skip_whitespace_chars(chars, j);
        if j >= chars.len() || chars[j] == '}' {
            break;
        }
        if chars[j] == '"' {
            let (key, key_end) = extract_json_string(chars, j);
            j = skip_to_char(chars, key_end, ':') + 1;
            j = skip_whitespace_chars(chars, j);
            if let Some((val, val_end)) = try_extract_string_value(chars, j) {
                match key.as_str() {
                    "src" => icon.src = val,
                    "sizes" => icon.sizes = val,
                    "type" => icon.icon_type = val,
                    "purpose" => icon.purpose = val,
                    _ => {}
                }
                j = val_end;
            } else {
                j = skip_json_value(chars, j);
            }
        } else {
            j += 1;
        }
        j = skip_whitespace_chars(chars, j);
        if j < chars.len() && chars[j] == ',' {
            j += 1;
        }
    }
    if j < chars.len() && chars[j] == '}' {
        j += 1;
    }
    (icon, j)
}

fn parse_css_color_value(s: &str) -> CssColor {
    let s = s.trim();
    // #rgb / #rrggbb / #rrggbbaa via the shared hex parser.
    if let Some(hex) = s.strip_prefix('#') {
        if let Some(c) = CssColor::from_hex(hex) {
            return c;
        }
    }
    // Named colors (rebeccapurple, dodgerblue, …) and `transparent`.
    if let Some(c) = CssColor::from_name(&s.to_ascii_lowercase()) {
        return c;
    }
    // rgb()/rgba()/hsl()/hsla() functional notation, via the same tokenizer the
    // main CSS path uses (so a manifest theme_color of `rgb(26,115,232)` resolves
    // rather than silently becoming white).
    let tokens = CssTokenizer::new(s).tokenize_all();
    if let CssValue::Color(c) = css_value_from_tokens(&tokens) {
        return c;
    }
    CssColor::WHITE
}

// ── PWA Cached Resource ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CachedResource {
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
    pub timestamp: u64,
}

/// CacheStorage — offline resource cache for PWA service workers.
/// Stores URL → response pairs for offline operation.
#[derive(Debug, Clone)]
pub struct CacheStorage {
    entries: Vec<CachedResource>,
    max_entries: usize,
    total_bytes: usize,
    max_bytes: usize,
}

impl CacheStorage {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            total_bytes: 0,
            max_bytes,
        }
    }

    pub fn put(&mut self, url: &str, content_type: &str, body: &[u8], timestamp: u64) -> bool {
        if self.total_bytes + body.len() > self.max_bytes {
            self.evict_oldest();
            if self.total_bytes + body.len() > self.max_bytes {
                return false;
            }
        }
        if self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }

        if let Some(existing) = self.entries.iter_mut().find(|e| e.url == url) {
            self.total_bytes -= existing.body.len();
            existing.body = body.to_vec();
            existing.content_type = String::from(content_type);
            existing.timestamp = timestamp;
            self.total_bytes += body.len();
        } else {
            self.entries.push(CachedResource {
                url: String::from(url),
                content_type: String::from(content_type),
                body: body.to_vec(),
                timestamp,
            });
            self.total_bytes += body.len();
        }
        true
    }

    pub fn get(&self, url: &str) -> Option<&CachedResource> {
        self.entries.iter().find(|e| e.url == url)
    }

    pub fn remove(&mut self, url: &str) -> bool {
        if let Some(idx) = self.entries.iter().position(|e| e.url == url) {
            self.total_bytes -= self.entries[idx].body.len();
            self.entries.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
    }

    pub fn contains(&self, url: &str) -> bool {
        self.entries.iter().any(|e| e.url == url)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn total_size(&self) -> usize {
        self.total_bytes
    }

    fn evict_oldest(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let oldest_idx = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, e)| e.timestamp)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.total_bytes -= self.entries[oldest_idx].body.len();
        self.entries.remove(oldest_idx);
    }
}

// ── PWA Service Worker ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceWorkerState {
    Installing,
    Installed,
    Activating,
    Activated,
    Redundant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchStrategy {
    CacheFirst,
    NetworkFirst,
    CacheOnly,
    NetworkOnly,
    StaleWhileRevalidate,
}

#[derive(Debug, Clone)]
pub struct FetchRoute {
    pub url_pattern: String,
    pub strategy: FetchStrategy,
}

pub struct PwaServiceWorker {
    pub state: ServiceWorkerState,
    pub scope: String,
    pub cache: CacheStorage,
    pub routes: Vec<FetchRoute>,
    pub precache_urls: Vec<String>,
}

impl PwaServiceWorker {
    pub fn new(scope: &str) -> Self {
        Self {
            state: ServiceWorkerState::Installing,
            scope: String::from(scope),
            cache: CacheStorage::new(512, 64 * 1024 * 1024),
            routes: Vec::new(),
            precache_urls: Vec::new(),
        }
    }

    pub fn install(&mut self, precache: &[&str]) {
        self.precache_urls = precache.iter().map(|u| String::from(*u)).collect();
        self.state = ServiceWorkerState::Installed;
    }

    pub fn activate(&mut self) {
        self.state = ServiceWorkerState::Activated;
    }

    pub fn add_route(&mut self, pattern: &str, strategy: FetchStrategy) {
        self.routes.push(FetchRoute {
            url_pattern: String::from(pattern),
            strategy,
        });
    }

    pub fn fetch(&self, url: &str) -> FetchStrategy {
        for route in &self.routes {
            if url_matches_pattern(url, &route.url_pattern) {
                return route.strategy;
            }
        }
        FetchStrategy::NetworkFirst
    }

    pub fn cache_response(&mut self, url: &str, content_type: &str, body: &[u8], timestamp: u64) {
        self.cache.put(url, content_type, body, timestamp);
    }

    pub fn get_cached(&self, url: &str) -> Option<&CachedResource> {
        self.cache.get(url)
    }

    pub fn is_active(&self) -> bool {
        self.state == ServiceWorkerState::Activated
    }
}

fn url_matches_pattern(url: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.ends_with('*') {
        let prefix = &pattern[..pattern.len() - 1];
        return url.starts_with(prefix);
    }
    if pattern.starts_with('*') {
        let suffix = &pattern[1..];
        return url.ends_with(suffix);
    }
    url == pattern
}

// ── PWA Notification ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct PwaNotification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub icon_url: String,
    pub tag: String,
    pub urgency: NotificationUrgency,
    pub timestamp: u64,
    pub app_id: u64,
    pub require_interaction: bool,
    pub silent: bool,
}

impl PwaNotification {
    pub fn new(id: u64, title: &str, body: &str, app_id: u64) -> Self {
        Self {
            id,
            title: String::from(title),
            body: String::from(body),
            icon_url: String::new(),
            tag: String::new(),
            urgency: NotificationUrgency::Normal,
            timestamp: 0,
            app_id,
            require_interaction: false,
            silent: false,
        }
    }
}

/// Bridge between web notifications and RaeShell notification daemon.
pub struct NotificationBridge {
    pending: Vec<PwaNotification>,
    next_id: u64,
    max_pending: usize,
}

impl NotificationBridge {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            next_id: 1,
            max_pending: 64,
        }
    }

    pub fn show(&mut self, title: &str, body: &str, app_id: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let notif = PwaNotification::new(id, title, body, app_id);
        if self.pending.len() < self.max_pending {
            self.pending.push(notif);
        }
        id
    }

    pub fn show_full(&mut self, mut notif: PwaNotification) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        notif.id = id;
        if self.pending.len() < self.max_pending {
            self.pending.push(notif);
        }
        id
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        if let Some(idx) = self.pending.iter().position(|n| n.id == id) {
            self.pending.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn dismiss_by_tag(&mut self, tag: &str) -> usize {
        let before = self.pending.len();
        self.pending.retain(|n| n.tag != tag);
        before - self.pending.len()
    }

    pub fn drain_pending(&mut self) -> Vec<PwaNotification> {
        core::mem::take(&mut self.pending)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

// ── PWA Window Frame ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowChrome {
    Full,
    Minimal,
    None,
}

impl WindowChrome {
    pub fn from_display_mode(mode: &PwaDisplayMode) -> Self {
        match mode {
            PwaDisplayMode::Browser => Self::Full,
            PwaDisplayMode::MinimalUi => Self::Minimal,
            PwaDisplayMode::Standalone => Self::Minimal,
            PwaDisplayMode::Fullscreen => Self::None,
        }
    }
}

pub struct PwaWindow {
    pub app_id: u64,
    pub title: String,
    pub chrome: WindowChrome,
    pub theme_color: CssColor,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub focused: bool,
    pub visible: bool,
    pub pipeline: RenderPipeline,
    pub current_url: String,
}

impl PwaWindow {
    pub fn new(app_id: u64, manifest: &PwaManifest, width: f32, height: f32) -> Self {
        let chrome = WindowChrome::from_display_mode(&manifest.display);
        let content_height = match chrome {
            WindowChrome::Full => height - 72.0,
            WindowChrome::Minimal => height - 32.0,
            WindowChrome::None => height,
        };
        Self {
            app_id,
            title: manifest.short_name.clone(),
            chrome,
            theme_color: manifest.theme_color,
            x: 0.0,
            y: 0.0,
            width,
            height,
            focused: true,
            visible: true,
            pipeline: RenderPipeline::new(width, content_height),
            current_url: manifest.start_url.clone(),
        }
    }

    pub fn render_content(&self, html: &str, css: &str) -> DisplayList {
        self.pipeline.render(html, css)
    }

    pub fn navigate(&mut self, url: &str) {
        self.current_url = String::from(url);
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = String::from(title);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.width = width;
        self.height = height;
        let content_height = match self.chrome {
            WindowChrome::Full => height - 72.0,
            WindowChrome::Minimal => height - 32.0,
            WindowChrome::None => height,
        };
        self.pipeline.resize(width, content_height);
    }

    pub fn chrome_height(&self) -> f32 {
        match self.chrome {
            WindowChrome::Full => 72.0,
            WindowChrome::Minimal => 32.0,
            WindowChrome::None => 0.0,
        }
    }

    pub fn content_rect(&self) -> Rect {
        let top = self.y + self.chrome_height();
        Rect {
            x: self.x,
            y: top,
            width: self.width,
            height: self.height - self.chrome_height(),
        }
    }
}

// ── PWA App ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PwaAppState {
    Installed,
    Running,
    Suspended,
    Uninstalled,
}

pub struct PwaApp {
    pub id: u64,
    pub manifest: PwaManifest,
    pub state: PwaAppState,
    pub service_worker: PwaServiceWorker,
    pub install_timestamp: u64,
    pub last_used: u64,
    pub data_usage_bytes: usize,
}

impl PwaApp {
    pub fn new(id: u64, manifest: PwaManifest, timestamp: u64) -> Self {
        let scope = manifest.scope.clone();
        Self {
            id,
            manifest,
            state: PwaAppState::Installed,
            service_worker: PwaServiceWorker::new(&scope),
            install_timestamp: timestamp,
            last_used: timestamp,
            data_usage_bytes: 0,
        }
    }

    pub fn launch(&mut self, timestamp: u64) {
        self.state = PwaAppState::Running;
        self.last_used = timestamp;
        if !self.service_worker.is_active() {
            self.service_worker.activate();
        }
    }

    pub fn suspend(&mut self) {
        if self.state == PwaAppState::Running {
            self.state = PwaAppState::Suspended;
        }
    }

    pub fn resume(&mut self, timestamp: u64) {
        if self.state == PwaAppState::Suspended {
            self.state = PwaAppState::Running;
            self.last_used = timestamp;
        }
    }

    pub fn is_within_scope(&self, url: &str) -> bool {
        url.starts_with(&self.manifest.scope)
    }
}

// ── PWA Manager ───────────────────────────────────────────────────────────

pub struct PwaManager {
    apps: Vec<PwaApp>,
    next_id: u64,
    notifications: NotificationBridge,
    max_apps: usize,
}

impl PwaManager {
    pub fn new() -> Self {
        Self {
            apps: Vec::new(),
            next_id: 1,
            notifications: NotificationBridge::new(),
            max_apps: 256,
        }
    }

    pub fn install(&mut self, manifest: PwaManifest, timestamp: u64) -> Option<u64> {
        if self.apps.len() >= self.max_apps {
            return None;
        }
        if self.find_by_start_url(&manifest.start_url).is_some() {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        let app = PwaApp::new(id, manifest, timestamp);
        self.apps.push(app);
        Some(id)
    }

    pub fn uninstall(&mut self, app_id: u64) -> bool {
        if let Some(idx) = self.apps.iter().position(|a| a.id == app_id) {
            self.apps[idx].state = PwaAppState::Uninstalled;
            self.apps.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn launch(&mut self, app_id: u64, timestamp: u64) -> bool {
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
            app.launch(timestamp);
            true
        } else {
            false
        }
    }

    pub fn get(&self, app_id: u64) -> Option<&PwaApp> {
        self.apps.iter().find(|a| a.id == app_id)
    }

    pub fn get_mut(&mut self, app_id: u64) -> Option<&mut PwaApp> {
        self.apps.iter_mut().find(|a| a.id == app_id)
    }

    pub fn list_installed(&self) -> Vec<(u64, &str)> {
        self.apps
            .iter()
            .filter(|a| a.state != PwaAppState::Uninstalled)
            .map(|a| (a.id, a.manifest.short_name.as_str()))
            .collect()
    }

    pub fn list_running(&self) -> Vec<u64> {
        self.apps
            .iter()
            .filter(|a| a.state == PwaAppState::Running)
            .map(|a| a.id)
            .collect()
    }

    pub fn find_by_start_url(&self, url: &str) -> Option<u64> {
        self.apps
            .iter()
            .find(|a| a.manifest.start_url == url)
            .map(|a| a.id)
    }

    pub fn notify(&mut self, app_id: u64, title: &str, body: &str) -> u64 {
        self.notifications.show(title, body, app_id)
    }

    pub fn drain_notifications(&mut self) -> Vec<PwaNotification> {
        self.notifications.drain_pending()
    }

    pub fn create_window(&self, app_id: u64, width: f32, height: f32) -> Option<PwaWindow> {
        self.get(app_id)
            .map(|app| PwaWindow::new(app_id, &app.manifest, width, height))
    }

    pub fn app_count(&self) -> usize {
        self.apps.len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  R10 CONTRACT — status, render smoketest, host KAT
//  "Native everywhere. No Electron tax." (Concept §Design Principles #1)
// ═══════════════════════════════════════════════════════════════════════════

/// Recursively count every node in a parsed DOM tree (Document + elements + text +
/// comments). The smoketest asserts this is non-zero — `dom_nodes == 0` is FAIL.
pub fn count_dom_nodes(node: &DomNode) -> usize {
    1 + node.children.iter().map(count_dom_nodes).sum::<usize>()
}

/// Count the element-bearing boxes in a laid-out tree (those with a tag name) —
/// the "laid_out" figure in the proof line.
pub fn count_layout_elements(layout: &LayoutBox) -> usize {
    let here = if layout.tag_name.is_some() { 1 } else { 0 };
    here + layout
        .children
        .iter()
        .map(count_layout_elements)
        .sum::<usize>()
}

/// Engine status for `/proc/raeen/web` (procfs-style line, R10).
///
/// Format: `engine=raeweb tabs=<n> last_url=<…> dom_nodes=<n> paint_cmds=<n> js=disabled`
#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub tabs: usize,
    pub last_url: String,
    pub dom_nodes: usize,
    pub paint_cmds: usize,
}

impl EngineStatus {
    pub fn render_line(&self) -> String {
        alloc::format!(
            "engine=raeweb tabs={} last_url={} dom_nodes={} paint_cmds={} js=disabled",
            self.tabs,
            if self.last_url.is_empty() {
                "-"
            } else {
                self.last_url.as_str()
            },
            self.dom_nodes,
            self.paint_cmds,
        )
    }
}

/// Structured outcome of [`run_render_smoketest`] — every field is an assertion the
/// caller (host KAT or boot smoketest) checks before declaring PASS.
#[derive(Debug, Clone)]
pub struct RenderSmoketest {
    /// The document was fetched over the real `raenet::http1` path.
    pub fetched: bool,
    pub dom_nodes: usize,
    pub styled: usize,
    pub laid_out: usize,
    /// Paint produced ≥1 command AND ≥1 `draw_text_aa` call.
    pub painted: bool,
    pub paint_stats: backend::PaintStats,
    /// Hit-testing the link's box returned an `<a>` node.
    pub link_hit: bool,
}

impl RenderSmoketest {
    /// All Phase-1 acceptance conditions met (the `-> PASS` gate). FAIL if the DOM
    /// is empty, paint emitted zero text calls, or the link hit-test missed.
    pub fn passed(&self) -> bool {
        self.fetched
            && self.dom_nodes > 0
            && self.laid_out > 0
            && self.painted
            && self.paint_stats.text_draws >= 1
            && self.link_hit
    }

    /// The house-style proof line.
    pub fn proof_line(&self) -> String {
        alloc::format!(
            "[web] render smoketest: fetched={} dom_nodes={} styled={} laid_out={} painted={} link_hit={} -> {}",
            if self.fetched { "ok" } else { "FAIL" },
            self.dom_nodes,
            self.styled,
            self.laid_out,
            if self.painted { "ok" } else { "FAIL" },
            if self.link_hit { "ok" } else { "FAIL" },
            if self.passed() { "PASS" } else { "FAIL" },
        )
    }
}

/// The Phase-1 render smoketest, parameterized over a paint sink so it runs in two
/// modes:
///   * host KAT / boot smoketest: paint into a real [`raegfx::Canvas`].
///
/// It fetches a trivial styled HTML doc (a heading + a link) over the **real**
/// `raenet::http1` client via the supplied transport, runs the full DOM → cascade →
/// layout → paint pipeline, paints through the canvas closure, and hit-tests the
/// link's box. Never panics: a malformed document degrades to `painted=false` /
/// `link_hit=false`, which the caller reports as FAIL.
pub fn run_render_smoketest<T, P>(transport: &mut T, mut paint: P) -> RenderSmoketest
where
    T: raenet::http1::HttpTransport,
    P: FnMut(&DisplayList) -> backend::PaintStats,
{
    // A trivial styled document: a heading and a link, with a tiny author sheet.
    const URL: &str = "http://localhost/index.html";
    const CSS: &str = "body { color: #222; font-size: 16px } \
                       h1 { font-size: 24px; font-weight: bold } \
                       a { color: #06c; display: block; height: 24px }";

    let pipeline = RenderPipeline::new(800.0, 600.0);

    // 1. Fetch over the real raenet path (transport may be a mock or a live socket).
    let (fetched, html) = match loader::fetch_document(URL, transport) {
        Ok(res) => (true, res.as_text()),
        Err(_) => (false, String::new()),
    };

    // 2/3. Parse + count (independent of layout so we report each stage honestly).
    let dom = parse_html(&html);
    let dom_nodes = count_dom_nodes(&dom);
    let sheet = parse_css(CSS);
    let mut order = 0u32;
    let styled_tree = build_styled_tree(&dom, &sheet, None, &mut order);
    let styled = count_styled_nodes(&styled_tree);

    // 4. Layout + paint.
    let layout = pipeline.render_to_layout(&html, CSS);
    let laid_out = count_layout_elements(&layout);
    let display_list = build_display_list(&layout, &pipeline.viewport);
    let paint_stats = paint(&display_list);
    let painted = !display_list.commands.is_empty() && paint_stats.text_draws >= 1;

    // 5. Hit-test the link's laid-out box and confirm it resolves to an <a>.
    let link_hit = find_box_for_tag(&layout, "a")
        .and_then(|rect| {
            let cx = rect.x + rect.width / 2.0;
            let cy = rect.y + rect.height / 2.0;
            hit_test(&layout, cx, cy)
        })
        .map(|hit| tag_chain_contains(&layout, hit.rect, "a"))
        .unwrap_or(false);

    RenderSmoketest {
        fetched,
        dom_nodes,
        styled,
        laid_out,
        painted,
        paint_stats,
        link_hit,
    }
}

/// Count the nodes in a styled tree (the "styled" figure in the proof line).
fn count_styled_nodes(node: &StyledNode) -> usize {
    1 + node.children.iter().map(count_styled_nodes).sum::<usize>()
}

/// Find the border-box [`Rect`] of the first layout box whose tag matches `tag`.
fn find_box_for_tag<'a>(layout: &'a LayoutBox, tag: &str) -> Option<Rect> {
    if layout.tag_name.as_deref() == Some(tag) {
        return Some(layout.dimensions.border_box());
    }
    for child in &layout.children {
        if let Some(r) = find_box_for_tag(child, tag) {
            return Some(r);
        }
    }
    None
}

/// True if any box overlapping `rect`'s center is (or contains) the given `tag` —
/// used to confirm a hit landed on the link's subtree, not a sibling.
fn tag_chain_contains(layout: &LayoutBox, rect: Rect, tag: &str) -> bool {
    let cx = rect.x + rect.width / 2.0;
    let cy = rect.y + rect.height / 2.0;
    for hit in hit_test_all(layout, cx, cy) {
        if hit.tag_name.as_deref() == Some(tag) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use raenet::http1::MockTransport;

    /// Build a canned HTTP/1.1 200 response carrying a trivial styled document with
    /// a heading and a link — the in-memory document for the deterministic KAT.
    fn canned_response() -> MockTransport {
        let body = "<!DOCTYPE html><html><body>\
                    <h1>RaeWeb</h1>\
                    <a href=\"/next\">Native everywhere</a>\
                    </body></html>";
        let resp = alloc::format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        MockTransport::new(resp.into_bytes())
    }

    /// Paint sink: a real raegfx Canvas backed by an owned buffer (so the test
    /// exercises the genuine draw_text_aa / fill_* path, not a counting stub).
    fn paint_into_owned_canvas(list: &DisplayList) -> backend::PaintStats {
        const W: usize = 800;
        const H: usize = 600;
        let mut buf = vec![0u8; W * H * 4];
        // SAFETY: `buf` outlives the Canvas (dropped at end of this fn); the canvas
        // only writes within W*H*4 bytes, which `buf` provides.
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), W, H, 4) };
        backend::paint_displaylist_to_canvas(list, &mut canvas, 0.0, 0.0)
    }

    #[test]
    fn render_smoketest_passes_full_pipeline() {
        let mut transport = canned_response();
        let result = run_render_smoketest(&mut transport, paint_into_owned_canvas);

        // The exact proof line the spec mandates:
        // [web] render smoketest: fetched=ok dom_nodes=8 styled=8 laid_out=4 \
        //   painted=ok link_hit=ok -> PASS
        let line = result.proof_line();
        assert!(line.ends_with("-> PASS"), "proof line not PASS: {}", line);

        // Each acceptance assertion, individually (so a failure pinpoints the stage).
        assert!(result.fetched, "fetch over raenet::http1 failed");
        assert!(result.dom_nodes > 0, "dom_nodes == 0");
        assert!(result.styled > 0, "styled == 0");
        assert!(result.laid_out > 0, "laid_out == 0");
        assert!(result.painted, "paint emitted no commands / no text");
        assert!(
            result.paint_stats.text_draws >= 1,
            "paint issued zero draw_text_aa calls: {:?}",
            result.paint_stats
        );
        assert!(result.link_hit, "hit-test over the link box missed the <a>");
    }

    /// FAIL-ability proof: feed an EMPTY document and assert the smoketest reports
    /// FAIL (no text painted, no link). A test that cannot fail is a false green.
    #[test]
    fn render_smoketest_fails_on_empty_document() {
        let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n";
        let mut transport = MockTransport::new(resp.as_bytes().to_vec());
        let result = run_render_smoketest(&mut transport, paint_into_owned_canvas);
        assert!(
            !result.passed(),
            "empty doc unexpectedly PASSed: {:?}",
            result
        );
        assert!(result.proof_line().ends_with("-> FAIL"));
        assert!(!result.link_hit, "empty doc should have no link to hit");
    }

    /// FAIL-ability proof #2: a transport that returns garbage must surface as a
    /// transport failure (`fetched == false`), never a panic.
    #[test]
    fn loader_reports_transport_failure_without_panicking() {
        let mut transport = MockTransport::new(b"not even http".to_vec());
        let result = run_render_smoketest(&mut transport, paint_into_owned_canvas);
        // Either the parse yields nothing useful or fetch failed; in both cases the
        // overall smoketest must NOT pass, and must not panic.
        assert!(!result.passed());
    }

    /// The paint bridge itself: a known display list must produce the expected
    /// command tally, and must clamp out-of-bounds geometry instead of panicking.
    #[test]
    fn paint_bridge_maps_commands_and_clamps() {
        let mut list = DisplayList::new();
        list.push(PaintCommand::FillRect {
            rect: Rect {
                x: 10.0,
                y: 10.0,
                width: 100.0,
                height: 40.0,
            },
            color: CssColor::rgb(200, 200, 200),
        });
        list.push(PaintCommand::FillText {
            text: String::from("hello"),
            x: 12.0,
            y: 14.0,
            font_size: 16.0,
            color: CssColor::BLACK,
            font_family: String::from("sans-serif"),
            font_weight: String::from("normal"),
            underline: false,
            strikethrough: false,
        });
        // Wildly out-of-bounds rect — must be skipped, not panic.
        list.push(PaintCommand::FillRect {
            rect: Rect {
                x: 1.0e9,
                y: 1.0e9,
                width: 50.0,
                height: 50.0,
            },
            color: CssColor::rgb(255, 0, 0),
        });

        let stats = paint_into_owned_canvas(&list);
        assert_eq!(stats.total_commands, 3);
        assert!(stats.rect_fills >= 1, "rect fill not issued");
        assert_eq!(stats.text_draws, 1, "text draw count wrong");
        assert!(stats.skipped >= 1, "out-of-bounds rect not skipped");
    }

    /// Malformed-HTML resilience: the tokenizer/tree-builder + bridge must never
    /// panic on junk. (docs/research/web-engine.md failure-modes: "never panic".)
    #[test]
    fn malformed_html_does_not_panic() {
        for junk in [
            "<<<>>><a href=",
            "<div><span></div></span><<!--",
            "<a><a><a><a href=\"x\">deep",
            "&#xZZZZ; &amp &; <style>}}}{{{",
            "",
        ] {
            let pipeline = RenderPipeline::new(400.0, 300.0);
            let layout = pipeline.render_to_layout(junk, "a{color:red}");
            let dl = build_display_list(&layout, &pipeline.viewport);
            let _ = paint_into_owned_canvas(&dl); // must not panic
        }
    }

    #[test]
    fn engine_status_line_format() {
        let s = EngineStatus {
            tabs: 1,
            last_url: String::from("http://localhost/index.html"),
            dom_nodes: 7,
            paint_cmds: 5,
        };
        let line = s.render_line();
        assert!(line.contains("engine=raeweb"));
        assert!(line.contains("js=disabled"));
        assert!(line.contains("dom_nodes=7"));
    }

    /// Link navigation: a click over the `<a>` box must resolve to its `href`, and a
    /// click elsewhere must resolve to `None` (the browser surface's navigate hook).
    #[test]
    fn link_href_at_resolves_clicked_anchor() {
        let html = "<!DOCTYPE html><html><body>\
                    <h1>RaeWeb</h1>\
                    <a href=\"/next\" style=\"display:block;height:24px\">Native everywhere</a>\
                    </body></html>";
        let css = "a { display: block; height: 24px }";
        let pipeline = RenderPipeline::new(800.0, 600.0);
        let dom = parse_html(html);
        let layout = pipeline.render_to_layout(html, css);

        // Find the anchor's laid-out box and click its center.
        let rect = find_box_for_tag(&layout, "a").expect("anchor must lay out");
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let href = loader::link_href_at(&dom, &layout, cx, cy);
        assert_eq!(href.as_deref(), Some("/next"), "click did not resolve href");

        // A click far outside any link resolves to None (FAIL-ability: a stub that
        // always returned the href would fail this).
        let miss = loader::link_href_at(&dom, &layout, 5000.0, 5000.0);
        assert!(miss.is_none(), "off-link click unexpectedly navigated");
    }

    // ── DomDocument: the mutable-DOM handle the JS binding reflects into ──────

    const MUT_DOC: &str = "<!DOCTYPE html><html><body>\
                           <p id=\"out\">old</p></body></html>";

    #[test]
    fn dom_document_reads_element_text() {
        let doc = DomDocument::parse(MUT_DOC, "", 400.0, 300.0);
        assert_eq!(doc.get_element_text("out").as_deref(), Some("old"));
        assert!(doc.has_element("out"));
        assert!(!doc.has_element("missing"));
        assert!(doc.get_element_text("missing").is_none());
    }

    #[test]
    fn dom_document_set_text_mutates_and_dirties_and_relays_out() {
        let mut doc = DomDocument::parse(MUT_DOC, "", 400.0, 300.0);
        assert!(!doc.is_dirty(), "fresh parse should be clean");

        let ok = doc.set_text_content("out", "new");
        assert!(ok, "writing to an existing id should land");
        assert!(doc.is_dirty(), "a mutation must mark the document dirty");

        // The read path now reflects the mutation.
        assert_eq!(doc.get_element_text("out").as_deref(), Some("new"));

        // And re-layout carries the NEW text (the whole point: the change shows).
        let layout = doc.render_to_layout();
        let text = first_box_text(&layout, "p").expect("the <p> must lay out");
        assert!(
            text.contains("new") && !text.contains("old"),
            "re-layout must reflect the mutated text, got {text:?}"
        );

        // take_dirty clears the flag.
        assert!(doc.take_dirty());
        assert!(!doc.is_dirty());
        assert!(!doc.take_dirty());
    }

    #[test]
    fn dom_document_missing_id_degrades_cleanly() {
        let mut doc = DomDocument::parse(MUT_DOC, "", 400.0, 300.0);
        // Writing to a non-existent id returns false and does NOT dirty.
        assert!(!doc.set_text_content("nope", "x"));
        assert!(!doc.is_dirty(), "a no-op write must not dirty the document");
    }

    #[test]
    fn dom_document_set_and_get_attribute() {
        let mut doc = DomDocument::parse(MUT_DOC, "", 400.0, 300.0);
        assert!(doc.get_attribute("out", "data-x").is_none());
        assert!(doc.set_attribute("out", "data-x", "42"));
        assert_eq!(doc.get_attribute("out", "data-x").as_deref(), Some("42"));
        assert!(doc.is_dirty());
        assert!(!doc.set_attribute("nope", "data-x", "1"));
    }

    /// Helper: text of the first laid-out box with the given tag.
    fn first_box_text(node: &LayoutBox, tag: &str) -> Option<String> {
        if node.tag_name.as_deref() == Some(tag) {
            let mut s = String::new();
            collect_layout_text(node, &mut s);
            if !s.is_empty() {
                return Some(s);
            }
        }
        for child in &node.children {
            if let Some(t) = first_box_text(child, tag) {
                return Some(t);
            }
        }
        None
    }

    fn collect_layout_text(node: &LayoutBox, out: &mut String) {
        if let Some(t) = &node.text {
            out.push_str(t);
        }
        for child in &node.children {
            collect_layout_text(child, out);
        }
    }

    /// Resolve the first color-valued declaration in a one-rule stylesheet.
    fn first_color(css: &str) -> CssColor {
        let sheet = parse_css(css);
        for rule in &sheet.rules {
            if let CssRule::Style(s) = rule {
                for d in &s.declarations {
                    if let CssValue::Color(c) = d.value {
                        return c;
                    }
                }
            }
        }
        panic!("no color declaration parsed from `{}`", css);
    }
    fn near(a: u8, b: u8) -> bool {
        (a as i32 - b as i32).abs() <= 1
    }

    #[test]
    fn css_hsl_color_parsing() {
        // Pure hues: hsl(0/120/240, 100%, 50%) == red / green / blue.
        let red = first_color("a { color: hsl(0, 100%, 50%); }");
        assert!(
            near(red.r, 255) && near(red.g, 0) && near(red.b, 0),
            "{red:?}"
        );
        let green = first_color("a { color: hsl(120, 100%, 50%); }");
        assert!(
            near(green.r, 0) && near(green.g, 255) && near(green.b, 0),
            "{green:?}"
        );
        let blue = first_color("a { color: hsl(240, 100%, 50%); }");
        assert!(
            near(blue.r, 0) && near(blue.g, 0) && near(blue.b, 255),
            "{blue:?}"
        );
        // Saturation 0 -> gray regardless of hue; l=50% -> ~128.
        let gray = first_color("a { color: hsl(123, 0%, 50%); }");
        assert!(
            near(gray.r, 128) && near(gray.g, 128) && near(gray.b, 128),
            "{gray:?}"
        );
        // Hue wraps (360deg == 0deg == red), proving the libm-free range-reduction.
        let wrap = first_color("a { color: hsl(360, 100%, 50%); }");
        assert!(
            near(wrap.r, 255) && near(wrap.g, 0) && near(wrap.b, 0),
            "{wrap:?}"
        );
        // hsla carries alpha.
        let semi = first_color("a { color: hsla(0, 100%, 50%, 0.5); }");
        assert!(near(semi.r, 255), "{semi:?}");
        assert!((semi.a - 0.5).abs() < 0.01, "alpha not 0.5: {}", semi.a);
    }

    #[test]
    fn manifest_color_value_parsing() {
        // Named color — previously fell back to white.
        let coral = parse_css_color_value("coral");
        assert_eq!((coral.r, coral.g, coral.b), (255, 127, 80));
        // 8-digit hex with alpha — previously unmatched -> white.
        let rgba = parse_css_color_value("#ff000080");
        assert_eq!((rgba.r, rgba.g, rgba.b), (255, 0, 0));
        assert!((rgba.a - 128.0 / 255.0).abs() < 0.01);
        // rgb() functional — previously -> white.
        let blue = parse_css_color_value("rgb(26, 115, 232)");
        assert_eq!((blue.r, blue.g, blue.b), (26, 115, 232));
        // 6-digit hex still resolves.
        let hex = parse_css_color_value("#1a73e8");
        assert_eq!((hex.r, hex.g, hex.b), (26, 115, 232));
        // An unparseable value still degrades to white (not a panic).
        let bad = parse_css_color_value("notacolor");
        assert_eq!((bad.r, bad.g, bad.b), (255, 255, 255));
    }

    /// First declaration value for `prop` across a parsed stylesheet (post var()
    /// resolution). Custom-property definitions use `--`-prefixed names, so asking
    /// for e.g. "color" returns the consumer's resolved value, not the token def.
    fn decl_value(css: &str, prop: &str) -> Option<CssValue> {
        let sheet = parse_css(css);
        for rule in &sheet.rules {
            if let CssRule::Style(s) = rule {
                for d in &s.declarations {
                    if d.property == prop {
                        return Some(d.value.clone());
                    }
                }
            }
        }
        None
    }

    #[test]
    fn css_custom_properties_var() {
        fn color_of(css: &str) -> CssColor {
            match decl_value(css, "color") {
                Some(CssValue::Color(c)) => c,
                other => panic!("expected a resolved color, got {other:?}"),
            }
        }
        // A :root design token resolved at a consumer declaration.
        let c = color_of(":root { --brand: #1a73e8; } a { color: var(--brand); }");
        assert_eq!((c.r, c.g, c.b), (26, 115, 232));
        // Undefined property -> the var() fallback value.
        let fb = color_of("a { color: var(--missing, #ff0000); }");
        assert_eq!((fb.r, fb.g, fb.b), (255, 0, 0));
        // var() -> var() chain resolves (bounded recursion).
        let chain = color_of(":root { --a: lime; --b: var(--a); } a { color: var(--b); }");
        assert_eq!((chain.r, chain.g, chain.b), (0, 255, 0));
        // var() inside a length value.
        let m = decl_value(":root { --gap: 12px; } a { margin: var(--gap); }", "margin");
        assert!(
            matches!(m, Some(CssValue::Length(v, _)) if (v - 12.0).abs() < 0.001),
            "{m:?}"
        );
        // Undefined with no fallback -> None (ignored, never mis-rendered).
        let none = decl_value("a { color: var(--nope); }", "color");
        assert!(matches!(none, Some(CssValue::None)), "{none:?}");
    }

    #[test]
    fn css_box_shorthand_expands_per_side() {
        fn find_box<'a>(node: &'a LayoutBox, tag: &str) -> Option<&'a LayoutBox> {
            if node.tag_name.as_deref() == Some(tag) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_box(c, tag) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // 4-value padding: top right bottom left (clockwise). Previously all 0.
        let lay = pipe.render_to_layout("<div></div>", "div { padding: 10px 20px 30px 40px; }");
        let p = &find_box(&lay, "div")
            .expect("div laid out")
            .dimensions
            .padding;
        assert_eq!((p.top, p.right, p.bottom, p.left), (10.0, 20.0, 30.0, 40.0));
        // 2-value margin: top/bottom = a, left/right = b.
        let lay2 = pipe.render_to_layout("<div></div>", "div { margin: 5px 15px; }");
        let m = &find_box(&lay2, "div").expect("div").dimensions.margin;
        assert_eq!((m.top, m.right, m.bottom, m.left), (5.0, 15.0, 5.0, 15.0));
        // 3-value padding: top = a, left/right = b, bottom = c.
        let lay3 = pipe.render_to_layout("<div></div>", "div { padding: 1px 2px 3px; }");
        let p3 = &find_box(&lay3, "div").expect("div").dimensions.padding;
        assert_eq!((p3.top, p3.right, p3.bottom, p3.left), (1.0, 2.0, 3.0, 2.0));
        // 1-value padding still uniform.
        let lay1 = pipe.render_to_layout("<div></div>", "div { padding: 7px; }");
        let p1 = &find_box(&lay1, "div").expect("div").dimensions.padding;
        assert_eq!((p1.top, p1.right, p1.bottom, p1.left), (7.0, 7.0, 7.0, 7.0));
    }

    #[test]
    fn html_named_entities_decode() {
        let doc = DomDocument::parse(
            "<div id=\"t\">caf&eacute;&mdash;&ldquo;x&rdquo;&times;&euro;&frac12;&rsquo;&uuml;&ntilde;</div>",
            "",
            400.0,
            300.0,
        );
        let t = doc.get_element_text("t").expect("text node");
        for needle in [
            "\u{00E9}", // eacute  é
            "\u{2014}", // mdash   —
            "\u{201C}", // ldquo   "
            "\u{201D}", // rdquo   "
            "\u{00D7}", // times   ×
            "\u{20AC}", // euro    €
            "\u{00BD}", // frac12  ½
            "\u{2019}", // rsquo   '
            "\u{00FC}", // uuml    ü
            "\u{00F1}", // ntilde  ñ
        ] {
            assert!(t.contains(needle), "entity {needle:?} missing from {t:?}");
        }
        // An unknown entity is left as a literal '&' (no panic, no drop).
        let raw = DomDocument::parse("<div id=\"u\">a&bogus;b</div>", "", 400.0, 300.0);
        assert!(raw.get_element_text("u").unwrap().contains('&'));
    }

    #[test]
    fn flex_wrap_breaks_lines() {
        fn find_id<'a>(n: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if n.node_id.as_deref() == Some(id) {
                return Some(n);
            }
            for c in &n.children {
                if let Some(b) = find_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        let html =
            "<div id=\"f\"><div id=\"a\"></div><div id=\"b\"></div><div id=\"c\"></div></div>";
        // Three 60px items in a 100px wrapping row: each fills its line, so b and
        // c wrap to new lines below a.
        let css = "#f { display: flex; flex-wrap: wrap; width: 100px; } \
                   #a, #b, #c { width: 60px; height: 20px; }";
        let lay = pipe.render_to_layout(html, css);
        let ay = find_id(&lay, "a").unwrap().dimensions.content.y;
        let by = find_id(&lay, "b").unwrap().dimensions.content.y;
        let cy = find_id(&lay, "c").unwrap().dimensions.content.y;
        assert!(by > ay, "b should wrap below a (ay={ay}, by={by})");
        assert!(cy > by, "c should wrap below b (by={by}, cy={cy})");
        // FAIL-ability: WITHOUT flex-wrap the three items stay on one line (same
        // y, overflowing the container) — so the wrap branch is load-bearing.
        let css2 = "#f { display: flex; width: 100px; } \
                    #a, #b, #c { width: 60px; height: 20px; }";
        let lay2 = pipe.render_to_layout(html, css2);
        let ay2 = find_id(&lay2, "a").unwrap().dimensions.content.y;
        let by2 = find_id(&lay2, "b").unwrap().dimensions.content.y;
        assert_eq!(ay2, by2, "no-wrap: b must stay on a's line");

        // justify-content applies PER wrapped line. Two 30px items on a 100px
        // line have 40px slack; `center` offsets the line's first item by 20px.
        let html2 = "<div id=\"f\"><div id=\"a\"></div><div id=\"b\"></div></div>";
        let css3 = "#f { display: flex; flex-wrap: wrap; justify-content: center; width: 100px; } \
                    #a, #b { width: 30px; height: 20px; }";
        let lay3 = pipe.render_to_layout(html2, css3);
        let ax_c = find_id(&lay3, "a").unwrap().dimensions.content.x;
        let css4 = "#f { display: flex; flex-wrap: wrap; width: 100px; } \
                    #a, #b { width: 30px; height: 20px; }";
        let lay4 = pipe.render_to_layout(html2, css4);
        let ax_l = find_id(&lay4, "a").unwrap().dimensions.content.x;
        assert!(
            ax_c > ax_l,
            "centered first item should sit right of flex-start (l={ax_l}, c={ax_c})"
        );
    }

    #[test]
    fn css_combinator_scoping() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        fn color_of(b: &LayoutBox) -> Option<(u8, u8, u8)> {
            match b.style.get("color") {
                Some(CssValue::Color(c)) => Some((c.r, c.g, c.b)),
                _ => None,
            }
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // Descendant `div p` styles ONLY the <p> inside a <div> — previously it
        // (wrongly) matched every <p> on the page.
        let lay = pipe.render_to_layout(
            "<div><p id=\"in\">x</p></div><p id=\"out\">y</p>",
            "div p { color: rgb(1,2,3); }",
        );
        assert_eq!(
            color_of(find_by_id(&lay, "in").expect("in")),
            Some((1, 2, 3))
        );
        assert_ne!(
            color_of(find_by_id(&lay, "out").expect("out")),
            Some((1, 2, 3)),
            "div p must NOT match a <p> outside any <div>"
        );
        // Child `section > p` matches a direct child, not a deeper descendant.
        let lay2 = pipe.render_to_layout(
            "<section><p id=\"direct\">a</p><div><p id=\"deep\">b</p></div></section>",
            "section > p { color: rgb(4,5,6); }",
        );
        assert_eq!(
            color_of(find_by_id(&lay2, "direct").expect("direct")),
            Some((4, 5, 6))
        );
        assert_ne!(
            color_of(find_by_id(&lay2, "deep").expect("deep")),
            Some((4, 5, 6)),
            "section > p must NOT match a <p> nested one level deeper"
        );
    }

    #[test]
    fn query_selector_all_respects_combinators() {
        // querySelectorAll('div p') returns only the <p> inside a <div>.
        let dom = parse_html("<div><p id=\"in\">x</p></div><p id=\"out\">y</p>");
        let ids: Vec<&str> = dom
            .query_selector_all("div p")
            .iter()
            .filter_map(|n| n.element_id())
            .collect();
        assert!(
            ids.contains(&"in"),
            "div p must match the in-div <p>: {ids:?}"
        );
        assert!(
            !ids.contains(&"out"),
            "div p must NOT match the <p> outside any div: {ids:?}"
        );
        // child combinator: section > p matches only the direct child.
        let dom2 =
            parse_html("<section><p id=\"direct\">a</p><div><p id=\"deep\">b</p></div></section>");
        let ids2: Vec<&str> = dom2
            .query_selector_all("section > p")
            .iter()
            .filter_map(|n| n.element_id())
            .collect();
        assert!(ids2.contains(&"direct"), "{ids2:?}");
        assert!(!ids2.contains(&"deep"), "{ids2:?}");
    }

    #[test]
    fn css_sibling_combinators() {
        let dom = parse_html(
            "<div><h1 id=\"h\">t</h1><p id=\"adj\">a</p><span></span><p id=\"far\">b</p></div>",
        );
        // Adjacent `h1 + p` matches ONLY the <p> immediately following the <h1>.
        let adj: Vec<&str> = dom
            .query_selector_all("h1 + p")
            .iter()
            .filter_map(|n| n.element_id())
            .collect();
        assert!(
            adj.contains(&"adj"),
            "h1 + p must match the immediately-following <p>: {adj:?}"
        );
        assert!(
            !adj.contains(&"far"),
            "h1 + p must NOT match a non-adjacent <p>: {adj:?}"
        );
        // General `h1 ~ p` matches every following sibling <p>.
        let gen: Vec<&str> = dom
            .query_selector_all("h1 ~ p")
            .iter()
            .filter_map(|n| n.element_id())
            .collect();
        assert!(
            gen.contains(&"adj") && gen.contains(&"far"),
            "h1 ~ p must match all following sibling <p>: {gen:?}"
        );
    }

    #[test]
    fn css_media_queries_apply_by_width() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        fn color_of(b: &LayoutBox) -> Option<(u8, u8, u8)> {
            match b.style.get("color") {
                Some(CssValue::Color(c)) => Some((c.r, c.g, c.b)),
                _ => None,
            }
        }
        let html = "<p id=\"t\">x</p>";
        let css = "p { color: rgb(0,0,0); } @media (max-width: 500px) { p { color: rgb(9,9,9); } }";
        // Narrow viewport (400 <= 500): the max-width media rule applies.
        let narrow = RenderPipeline::new(400.0, 600.0).render_to_layout(html, css);
        assert_eq!(
            color_of(find_by_id(&narrow, "t").expect("t")),
            Some((9, 9, 9))
        );
        // Wide viewport (800 > 500): the media rule is dropped; the base rule wins.
        let wide = RenderPipeline::new(800.0, 600.0).render_to_layout(html, css);
        assert_eq!(
            color_of(find_by_id(&wide, "t").expect("t")),
            Some((0, 0, 0))
        );
        // min-width matches the other direction.
        let css2 =
            "p { color: rgb(0,0,0); } @media (min-width: 700px) { p { color: rgb(1,1,1); } }";
        let wide2 = RenderPipeline::new(800.0, 600.0).render_to_layout(html, css2);
        assert_eq!(
            color_of(find_by_id(&wide2, "t").expect("t")),
            Some((1, 1, 1))
        );
    }

    #[test]
    fn css_text_transform_applies() {
        fn all_text(node: &LayoutBox) -> String {
            let mut s = String::new();
            collect_layout_text(node, &mut s);
            s
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // uppercase / lowercase are per-character, so they're correct even though
        // raeweb fragments text into per-character nodes.
        let up = pipe.render_to_layout("<p>Hello World</p>", "p { text-transform: uppercase; }");
        assert!(all_text(&up).contains("HELLO WORLD"), "{:?}", all_text(&up));
        let low = pipe.render_to_layout("<p>Hello World</p>", "p { text-transform: lowercase; }");
        assert!(
            all_text(&low).contains("hello world"),
            "{:?}",
            all_text(&low)
        );
        // capitalize now works: text runs are coalesced, so word boundaries exist.
        let cap = pipe.render_to_layout("<p>hello world</p>", "p { text-transform: capitalize; }");
        assert!(
            all_text(&cap).contains("Hello World"),
            "{:?}",
            all_text(&cap)
        );
        // No transform leaves the text unchanged.
        let plain = pipe.render_to_layout("<p>Hello World</p>", "p { color: red; }");
        assert!(
            all_text(&plain).contains("Hello World"),
            "{:?}",
            all_text(&plain)
        );
    }

    #[test]
    fn css_box_sizing_border_box() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // content-box (default): content width == the specified width.
        let cb = pipe.render_to_layout(
            "<div id=\"b\">x</div>",
            "div { width: 200px; padding: 20px; }",
        );
        let w_cb = find_by_id(&cb, "b").expect("b").dimensions.content.width;
        assert!((w_cb - 200.0).abs() < 0.001, "content-box width = {w_cb}");
        // border-box: content width = 200 - padding(20+20) = 160.
        let bb = pipe.render_to_layout(
            "<div id=\"b\">x</div>",
            "div { width: 200px; padding: 20px; box-sizing: border-box; }",
        );
        let w_bb = find_by_id(&bb, "b").expect("b").dimensions.content.width;
        assert!((w_bb - 160.0).abs() < 0.001, "border-box width = {w_bb}");
        // border-box also subtracts borders: 200 - padding(10+10) - border(5+5) = 170.
        let bb2 = pipe.render_to_layout(
            "<div id=\"b\">x</div>",
            "div { width: 200px; padding: 10px; border-width: 5px; box-sizing: border-box; }",
        );
        let w_bb2 = find_by_id(&bb2, "b").expect("b").dimensions.content.width;
        assert!(
            (w_bb2 - 170.0).abs() < 0.001,
            "border-box w/border = {w_bb2}"
        );
    }

    #[test]
    fn css_white_space_nowrap() {
        fn find_text_box<'a>(b: &'a LayoutBox) -> Option<&'a LayoutBox> {
            if b.text.is_some() {
                return Some(b);
            }
            for c in &b.children {
                if let Some(x) = find_text_box(c) {
                    return Some(x);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(100.0, 600.0); // narrow viewport forces wrapping
        let html = alloc::format!("<p>{}</p>", "a".repeat(40));
        // default: text wraps across several lines -> tall.
        let wrap = pipe.render_to_layout(&html, "p { font-size: 16px; }");
        let h_wrap = find_text_box(&wrap).expect("t").dimensions.content.height;
        // nowrap: single line -> short, regardless of width.
        let nowrap = pipe.render_to_layout(&html, "p { font-size: 16px; white-space: nowrap; }");
        let h_nowrap = find_text_box(&nowrap).expect("t").dimensions.content.height;
        assert!(
            h_nowrap < h_wrap,
            "nowrap height {h_nowrap} must be < wrapped height {h_wrap}"
        );
        // nowrap is exactly one line (line-height 1.2 * 16px).
        assert!(
            (h_nowrap - 16.0 * 1.2).abs() < 1.0,
            "nowrap should be a single line, got {h_nowrap}"
        );
    }

    #[test]
    fn css_calc_values() {
        fn width_val(css: &str) -> CssValue {
            let sheet = parse_css(css);
            match &sheet.rules[0] {
                CssRule::Style(sr) => sr
                    .declarations
                    .iter()
                    .find(|d| d.property == "width")
                    .map(|d| d.value.clone())
                    .expect("width decl"),
                _ => panic!("expected a style rule"),
            }
        }
        // Absolute arithmetic folds to a single Length.
        assert!(matches!(width_val("a { width: calc(50px + 30px); }"),
            CssValue::Length(v, _) if (v - 80.0).abs() < 0.01));
        assert!(
            matches!(width_val("a { width: calc(100px - 30px - 20px); }"),
            CssValue::Length(v, _) if (v - 50.0).abs() < 0.01)
        );
        assert!(matches!(width_val("a { width: calc(2 * 25px); }"),
            CssValue::Length(v, _) if (v - 50.0).abs() < 0.01));
        assert!(matches!(width_val("a { width: calc(60px / 2); }"),
            CssValue::Length(v, _) if (v - 30.0).abs() < 0.01));
        // Precedence: * binds tighter than + (10 + 2*5 = 20, not 60).
        assert!(matches!(width_val("a { width: calc(10px + 2 * 5px); }"),
            CssValue::Length(v, _) if (v - 20.0).abs() < 0.01));
        // Pure percentage folds to Percentage.
        assert!(matches!(width_val("a { width: calc(100% / 4); }"),
            CssValue::Percentage(v) if (v - 25.0).abs() < 0.01));
        // Mixed px + % -> Calc { px, pct, .. }.
        assert!(matches!(width_val("a { width: calc(100% - 40px); }"),
            CssValue::Calc { px, pct, .. } if (px + 40.0).abs() < 0.01 && (pct - 100.0).abs() < 0.01));
        // `rem` (×16) and `pt` (×1.333) are constants → fold straight into px.
        assert!(matches!(width_val("a { width: calc(2rem + 8px); }"),
            CssValue::Length(v, _) if (v - 40.0).abs() < 0.01));
        // `em` is font-size-relative → kept as an em term in the linear Calc.
        assert!(matches!(width_val("a { width: calc(10em + 5px); }"),
            CssValue::Calc { px, em, .. } if (px - 5.0).abs() < 0.01 && (em - 10.0).abs() < 0.01));
        // `vw`/`vh` are viewport-relative → kept as vw/vh terms.
        assert!(matches!(width_val("a { width: calc(100vh - 60px); }"),
            CssValue::Calc { px, vh, .. } if (px + 60.0).abs() < 0.01 && (vh - 100.0).abs() < 0.01));
        // em term resolves at layout: 10em at 16px font-size + 5px = 165px.
        assert!(
            (width_val("a { width: calc(10em + 5px); }").to_px(16.0, 1000.0, 800.0) - 165.0).abs()
                < 0.01
        );
        // vh term resolves against the viewport: 100vh of an 800px viewport - 60px.
        assert!(
            (width_val("a { width: calc(100vh - 60px); }").to_px(16.0, 1000.0, 800.0) - 740.0)
                .abs()
                < 0.01
        );
        // A still-unsupported unit (ch) -> left unevaluated as a Function.
        assert!(matches!(width_val("a { width: calc(10ch + 5px); }"),
            CssValue::Function(name, _) if name == "calc"));

        // A mixed calc RESOLVES against the container at layout: calc(100% - 100px)
        // is exactly 100px narrower than a plain 100% (container-agnostic check).
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(300.0, 600.0);
        let calc = pipe.render_to_layout(
            "<div id=\"x\">y</div>",
            "div { width: calc(100% - 100px); }",
        );
        let full = pipe.render_to_layout("<div id=\"x\">y</div>", "div { width: 100%; }");
        let wc = find_by_id(&calc, "x").expect("x").dimensions.content.width;
        let wf = find_by_id(&full, "x").expect("x").dimensions.content.width;
        assert!(
            (wc - (wf - 100.0)).abs() < 0.5,
            "calc(100%-100px)={wc} vs 100%={wf}"
        );
    }

    #[test]
    fn css_flex_grow_distributes_space() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        let html = "<div class=\"row\"><div class=\"cell\" id=\"a\">x</div><div class=\"cell\" id=\"b\">y</div></div>";
        // Equal flex-grow: base 40 each (total 80) in a 300px row -> remaining 220
        // split equally -> each 40 + 110 = 150.
        let css = ".row { display: flex; width: 300px; } .cell { width: 40px; flex-grow: 1; }";
        let lay = pipe.render_to_layout(html, css);
        let wa = find_by_id(&lay, "a").expect("a").dimensions.content.width;
        let wb = find_by_id(&lay, "b").expect("b").dimensions.content.width;
        assert!((wa - 150.0).abs() < 1.0, "a width = {wa}");
        assert!((wb - 150.0).abs() < 1.0, "b width = {wb}");
        // Unequal grow: a:2 b:1 split the 220 remaining as 2/3 vs 1/3.
        let css2 = ".row { display: flex; width: 300px; } #a { width: 40px; flex-grow: 2; } #b { width: 40px; flex-grow: 1; }";
        let lay2 = pipe.render_to_layout(html, css2);
        let wa2 = find_by_id(&lay2, "a").expect("a").dimensions.content.width;
        let wb2 = find_by_id(&lay2, "b").expect("b").dimensions.content.width;
        assert!((wa2 - (40.0 + 220.0 * 2.0 / 3.0)).abs() < 1.0, "a2 = {wa2}");
        assert!((wb2 - (40.0 + 220.0 / 3.0)).abs() < 1.0, "b2 = {wb2}");
    }

    #[test]
    fn css_text_align_positions_line() {
        fn find_text_box<'a>(b: &'a LayoutBox) -> Option<&'a LayoutBox> {
            if b.text.is_some() {
                return Some(b);
            }
            for c in &b.children {
                if let Some(x) = find_text_box(c) {
                    return Some(x);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        let x = |css: &str| -> f32 {
            let lay = pipe.render_to_layout("<p>hi</p>", css);
            find_text_box(&lay).expect("text box").dimensions.content.x
        };
        // A short single line: center shifts it right of left; right shifts further.
        let x_left = x("p { text-align: left; font-size: 16px; }");
        let x_center = x("p { text-align: center; font-size: 16px; }");
        let x_right = x("p { text-align: right; font-size: 16px; }");
        assert!(
            x_center > x_left + 50.0,
            "center x {x_center} must exceed left x {x_left}"
        );
        assert!(
            x_right > x_center + 50.0,
            "right x {x_right} must exceed center x {x_center}"
        );
    }

    #[test]
    fn css_text_decoration_underline() {
        fn first_text_underline(list: &DisplayList) -> Option<bool> {
            for cmd in &list.commands {
                if let PaintCommand::FillText {
                    text, underline, ..
                } = cmd
                {
                    if !text.trim().is_empty() {
                        return Some(*underline);
                    }
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // <a> is underlined by default (its default style sets text-decoration).
        let link = pipe.render("<a href=\"/x\">link</a>", "");
        assert_eq!(
            first_text_underline(&link),
            Some(true),
            "links must underline"
        );
        // Explicit text-decoration: underline.
        let u = pipe.render("<p>word</p>", "p { text-decoration: underline; }");
        assert_eq!(first_text_underline(&u), Some(true));
        // Plain text is not underlined.
        let plain = pipe.render("<p>word</p>", "p { text-decoration: none; }");
        assert_eq!(first_text_underline(&plain), Some(false));
    }

    #[test]
    fn css_list_markers() {
        fn texts(list: &DisplayList) -> alloc::vec::Vec<String> {
            list.commands
                .iter()
                .filter_map(|c| match c {
                    PaintCommand::FillText { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // <ul> -> one disc bullet per <li>.
        let ul = texts(&pipe.render("<ul><li>a</li><li>b</li></ul>", ""));
        assert_eq!(
            ul.iter().filter(|t| t.contains('•')).count(),
            2,
            "two <li> -> two bullets: {ul:?}"
        );
        // <ol> -> ascending ordinals.
        let ol = texts(&pipe.render("<ol><li>a</li><li>b</li><li>c</li></ol>", ""));
        assert!(ol.iter().any(|t| t == "1."), "ol must have 1.: {ol:?}");
        assert!(ol.iter().any(|t| t == "2."), "ol must have 2.: {ol:?}");
        assert!(ol.iter().any(|t| t == "3."), "ol must have 3.: {ol:?}");
        // list-style-type: none suppresses the marker (inherited to the <li>).
        let none = texts(&pipe.render("<ul style=\"list-style-type:none\"><li>a</li></ul>", ""));
        assert_eq!(
            none.iter().filter(|t| t.contains('•')).count(),
            0,
            "{none:?}"
        );
    }

    #[test]
    fn css_visibility_hidden() {
        fn has_text(list: &DisplayList, needle: &str) -> bool {
            list.commands
                .iter()
                .any(|c| matches!(c, PaintCommand::FillText { text, .. } if text.contains(needle)))
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // Default: text paints.
        assert!(has_text(&pipe.render("<p>shown</p>", ""), "shown"));
        // visibility: hidden -> the text does NOT paint (box still takes space).
        assert!(!has_text(
            &pipe.render("<p>gone</p>", "p { visibility: hidden; }"),
            "gone"
        ));
        // A visibility: visible descendant overrides a hidden ancestor.
        let ov = pipe.render(
            "<div><span>back</span></div>",
            "div { visibility: hidden; } span { visibility: visible; }",
        );
        assert!(
            has_text(&ov, "back"),
            "visible child must override hidden parent"
        );
    }

    #[test]
    fn css_text_decoration_strikethrough() {
        fn find<'a>(list: &'a DisplayList, needle: &str) -> Option<&'a PaintCommand> {
            list.commands
                .iter()
                .find(|c| matches!(c, PaintCommand::FillText { text, .. } if text.contains(needle)))
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // Explicit line-through strikes (and does not underline).
        let lt = pipe.render("<p>old</p>", "p { text-decoration: line-through; }");
        assert!(matches!(
            find(&lt, "old"),
            Some(PaintCommand::FillText {
                strikethrough: true,
                underline: false,
                ..
            })
        ));
        // <del> strikes through by default.
        let del = pipe.render("<del>removed</del>", "");
        assert!(matches!(
            find(&del, "removed"),
            Some(PaintCommand::FillText {
                strikethrough: true,
                ..
            })
        ));
        // <u> underlines by default (its own default arm).
        let u = pipe.render("<u>under</u>", "");
        assert!(matches!(
            find(&u, "under"),
            Some(PaintCommand::FillText {
                underline: true,
                ..
            })
        ));
        // Plain text: neither decoration.
        let plain = pipe.render("<p>plain</p>", "");
        assert!(matches!(
            find(&plain, "plain"),
            Some(PaintCommand::FillText {
                underline: false,
                strikethrough: false,
                ..
            })
        ));
    }

    #[test]
    fn css_linear_gradient_background() {
        fn gradient(list: &DisplayList) -> Option<(CssColor, CssColor)> {
            list.commands.iter().find_map(|c| match c {
                PaintCommand::FillGradient { top, bottom, .. } => Some((*top, *bottom)),
                _ => None,
            })
        }
        fn solid(list: &DisplayList) -> Option<CssColor> {
            list.commands.iter().find_map(|c| match c {
                PaintCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
        }
        let pipe = RenderPipeline::new(200.0, 200.0);
        // Default direction -> vertical, first color on top.
        let g = pipe.render(
            "<div>x</div>",
            "div { background: linear-gradient(#010000, #000001); }",
        );
        let (top, bottom) = gradient(&g).expect("vertical gradient");
        assert_eq!((top.r, top.b), (1, 0), "top = first color");
        assert_eq!((bottom.r, bottom.b), (0, 1), "bottom = last color");
        // `to top` reverses the stops.
        let gt = pipe.render(
            "<div>x</div>",
            "div { background: linear-gradient(to top, #010000, #000001); }",
        );
        let (t2, _b2) = gradient(&gt).expect("gradient");
        assert_eq!((t2.r, t2.b), (0, 1), "to top -> last color on top");
        // Horizontal is unsupported -> solid first color, no gradient.
        let gh = pipe.render(
            "<div>x</div>",
            "div { background: linear-gradient(to right, #010000, #000001); }",
        );
        assert!(gradient(&gh).is_none(), "horizontal -> no FillGradient");
        assert!(
            solid(&gh).map_or(false, |c| c.r == 1 && c.b == 0),
            "horizontal -> solid first color"
        );
    }

    #[test]
    fn css_nested_function_values() {
        fn gradient(list: &DisplayList) -> Option<(CssColor, CssColor)> {
            list.commands.iter().find_map(|c| match c {
                PaintCommand::FillGradient { top, bottom, .. } => Some((*top, *bottom)),
                _ => None,
            })
        }
        let pipe = RenderPipeline::new(200.0, 200.0);
        // Nested rgb() inside linear-gradient now resolves (was lost before the
        // recursive arg parse).
        let g = pipe.render(
            "<div>x</div>",
            "div { background: linear-gradient(rgb(10,20,30), rgb(40,50,60)); }",
        );
        let (top, bottom) = gradient(&g).expect("nested-rgb gradient");
        assert_eq!((top.r, top.g, top.b), (10, 20, 30));
        assert_eq!((bottom.r, bottom.g, bottom.b), (40, 50, 60));
        // Nested hsl() resolves too: hsl(0,100%,50%) = red, hsl(240,100%,50%) = blue.
        let g2 = pipe.render(
            "<div>x</div>",
            "div { background: linear-gradient(hsl(0,100%,50%), hsl(240,100%,50%)); }",
        );
        let (t2, b2) = gradient(&g2).expect("nested-hsl gradient");
        assert_eq!((t2.r, t2.g, t2.b), (255, 0, 0), "first stop is red");
        assert!(b2.b > 250 && b2.r < 5, "second stop is blue: {b2:?}");
    }

    #[test]
    fn html_script_style_rawtext() {
        fn elem_text(node: &DomNode, tag: &str) -> Option<String> {
            if node.tag_name() == Some(tag) {
                let mut s = String::new();
                for c in &node.children {
                    if let NodeType::Text(t) = &c.node_type {
                        s.push_str(t);
                    }
                }
                return Some(s);
            }
            for c in &node.children {
                if let Some(t) = elem_text(c, tag) {
                    return Some(t);
                }
            }
            None
        }
        // A `<` inside a script (JS comparison) must NOT open a tag; the whole body
        // is one raw text node, entities undecoded.
        let dom = parse_html("<div><script>if (a < b && c > d) { x() }</script></div>");
        let js = elem_text(&dom, "script").expect("script element");
        assert!(js.contains("a < b"), "raw '<' preserved: {js:?}");
        assert!(js.contains("c > d"), "raw '>' preserved: {js:?}");
        assert!(js.contains("&&"), "entities not decoded in rawtext: {js:?}");
        // <style> is rawtext too, and the </style> close still ends it.
        let dom2 = parse_html("<style>a > b { color: red }</style><p>after</p>");
        let css = elem_text(&dom2, "style").expect("style element");
        assert!(css.contains("a > b"), "style raw content: {css:?}");
        // The element after the closed rawtext still parses normally.
        assert!(elem_text(&dom2, "p").map_or(false, |t| t.contains("after")));
    }

    #[test]
    fn html_implicit_tag_closing() {
        fn count_child_tags(node: &DomNode, tag: &str, child: &str) -> Option<usize> {
            if node.tag_name() == Some(tag) {
                return Some(
                    node.children
                        .iter()
                        .filter(|c| c.tag_name() == Some(child))
                        .count(),
                );
            }
            for c in &node.children {
                if let Some(n) = count_child_tags(c, tag, child) {
                    return Some(n);
                }
            }
            None
        }
        // Unclosed <li>s are SIBLINGS under <ul>, not nested.
        let dom = parse_html("<ul><li>a<li>b<li>c</ul>");
        assert_eq!(
            count_child_tags(&dom, "ul", "li"),
            Some(3),
            "three sibling <li>"
        );
        // A block start tag implicitly closes an open <p>, so <div> is NOT inside <p>.
        let dom2 = parse_html("<p>text<div>more</div></p>");
        assert_eq!(
            count_child_tags(&dom2, "p", "div"),
            Some(0),
            "<div> must not nest inside <p>"
        );
        // <dt>/<dd> close each other.
        let dom3 = parse_html("<dl><dt>term<dd>def<dt>term2</dl>");
        assert_eq!(count_child_tags(&dom3, "dl", "dt"), Some(2));
        assert_eq!(count_child_tags(&dom3, "dl", "dd"), Some(1));
    }

    #[test]
    fn css_at_rule_skipping() {
        fn colors(sheet: &CssStylesheet) -> alloc::vec::Vec<(u8, u8, u8)> {
            let mut v = alloc::vec::Vec::new();
            for rule in &sheet.rules {
                if let CssRule::Style(sr) = rule {
                    for d in &sr.declarations {
                        if d.property == "color" {
                            if let CssValue::Color(c) = d.value {
                                v.push((c.r, c.g, c.b));
                            }
                        }
                    }
                }
            }
            v
        }
        // @import is a STATEMENT at-rule (no block) ending at ';' — it must not
        // swallow the following style rule (the bug before the depth-0 ';' break).
        let s1 = parse_css("@import \"base.css\"; p { color: rgb(1,2,3); }");
        assert!(
            colors(&s1).contains(&(1, 2, 3)),
            "p rule survives @import: {:?}",
            colors(&s1)
        );
        // @keyframes is a BLOCK at-rule with NESTED braces — skipped wholesale.
        let s2 = parse_css(
            "@keyframes spin { from { opacity: 0; } to { opacity: 1; } } p { color: rgb(4,5,6); }",
        );
        assert!(
            colors(&s2).contains(&(4, 5, 6)),
            "p rule survives @keyframes: {:?}",
            colors(&s2)
        );
        // @font-face block skipped too.
        let s3 = parse_css("@font-face { font-family: x; src: url(a); } p { color: rgb(7,8,9); }");
        assert!(
            colors(&s3).contains(&(7, 8, 9)),
            "p rule survives @font-face: {:?}",
            colors(&s3)
        );
    }

    #[test]
    fn css_border_shorthand() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // `border: 3px solid rgb(1,2,3)` expands to width/style/color longhands.
        let lay = pipe.render_to_layout(
            "<div id=\"b\">x</div>",
            "div { border: 3px solid rgb(1,2,3); }",
        );
        let bx = find_by_id(&lay, "b").expect("b");
        assert!(
            (bx.dimensions.border.top - 3.0).abs() < 0.01
                && (bx.dimensions.border.left - 3.0).abs() < 0.01,
            "border width = {}/{}",
            bx.dimensions.border.top,
            bx.dimensions.border.left
        );
        assert!(matches!(bx.style.get("border-style"), Some(CssValue::Keyword(k)) if k == "solid"));
        assert!(matches!(
            bx.style.get("border-color"),
            Some(CssValue::Color(c)) if c.r == 1 && c.g == 2 && c.b == 3
        ));
        // A longhand AFTER the shorthand overrides it (cascade order).
        let lay2 = pipe.render_to_layout(
            "<div id=\"b\">x</div>",
            "div { border: 3px solid red; border-width: 7px; }",
        );
        let b2 = find_by_id(&lay2, "b").expect("b");
        assert!(
            (b2.dimensions.border.top - 7.0).abs() < 0.01,
            "longhand overrides shorthand: {}",
            b2.dimensions.border.top
        );
    }

    #[test]
    fn css_background_shorthand_color() {
        fn first_rect(list: &DisplayList) -> Option<CssColor> {
            list.commands.iter().find_map(|c| match c {
                PaintCommand::FillRect { color, .. } => Some(*color),
                _ => None,
            })
        }
        let pipe = RenderPipeline::new(200.0, 200.0);
        // `background` shorthand carrying a color among other tokens still paints it
        // (the nested rgb() resolves; the Multiple's color is extracted).
        let g = pipe.render(
            "<div>x</div>",
            "div { background: rgb(10,20,30) no-repeat; }",
        );
        let c = first_rect(&g).expect("bg rect");
        assert_eq!((c.r, c.g, c.b), (10, 20, 30));
    }

    #[test]
    fn css_font_shorthand() {
        fn find_by_tag<'a>(node: &'a LayoutBox, tag: &str) -> Option<&'a LayoutBox> {
            if node.tag_name.as_deref() == Some(tag) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_tag(c, tag) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // `font: italic bold 20px Georgia` -> style/weight/size/family longhands.
        let lay = pipe.render_to_layout("<p>hi</p>", "p { font: italic bold 20px Georgia; }");
        let p = find_by_tag(&lay, "p").expect("p");
        assert!(
            matches!(p.style.get("font-size"), Some(CssValue::Length(v, _)) if (*v - 20.0).abs() < 0.01)
        );
        assert!(matches!(p.style.get("font-weight"), Some(CssValue::Keyword(k)) if k == "bold"));
        assert!(matches!(p.style.get("font-style"), Some(CssValue::Keyword(k)) if k == "italic"));
        assert!(
            matches!(p.style.get("font-family"), Some(CssValue::Raw(f)) if f.contains("Georgia"))
        );
        // A longhand after the shorthand overrides it.
        let lay2 =
            pipe.render_to_layout("<p>hi</p>", "p { font: bold 20px Arial; font-size: 30px; }");
        let p2 = find_by_tag(&lay2, "p").expect("p");
        assert!(
            matches!(p2.style.get("font-size"), Some(CssValue::Length(v, _)) if (*v - 30.0).abs() < 0.01)
        );
    }

    #[test]
    fn css_inherit_initial_keywords() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // background-color is NOT inherited by default; explicit `inherit` pulls the
        // parent's value.
        let lay = pipe.render_to_layout(
            "<div><span id=\"c\">x</span></div>",
            "div { background-color: rgb(1,2,3); } span { background-color: inherit; }",
        );
        let c = find_by_id(&lay, "c").expect("c");
        assert!(matches!(
            c.style.get("background-color"),
            Some(CssValue::Color(col)) if col.r == 1 && col.g == 2 && col.b == 3
        ));
        // `initial` RESETS — the child does not take the parent's (inherited) color.
        let lay2 = pipe.render_to_layout(
            "<div><span id=\"c\">x</span></div>",
            "div { color: rgb(9,9,9); } span { color: initial; }",
        );
        let c2 = find_by_id(&lay2, "c").expect("c");
        assert!(
            !matches!(c2.style.get("color"), Some(CssValue::Color(col)) if col.r == 9),
            "initial must reset, not inherit"
        );
    }

    #[test]
    fn css_border_default_currentcolor() {
        fn border_color(list: &DisplayList) -> Option<CssColor> {
            list.commands.iter().find_map(|c| match c {
                PaintCommand::DrawBorder { color, .. } => Some(*color),
                _ => None,
            })
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        // A border with no explicit color uses the element's `color` (currentColor),
        // not black.
        let g = pipe.render("<p>x</p>", "p { color: rgb(5,6,7); border: 2px solid; }");
        let bc = border_color(&g).expect("border");
        assert_eq!(
            (bc.r, bc.g, bc.b),
            (5, 6, 7),
            "default border-color is currentColor"
        );
        // explicit currentColor keyword resolves the same way.
        let g2 = pipe.render(
            "<p>x</p>",
            "p { color: rgb(8,9,10); border: 2px solid currentColor; }",
        );
        let bc2 = border_color(&g2).expect("border");
        assert_eq!((bc2.r, bc2.g, bc2.b), (8, 9, 10));
    }

    #[test]
    fn css_text_shadow() {
        fn text_draws(list: &DisplayList) -> alloc::vec::Vec<(f32, f32, CssColor)> {
            list.commands
                .iter()
                .filter_map(|c| match c {
                    PaintCommand::FillText {
                        x, y, color, text, ..
                    } if !text.trim().is_empty() => Some((*x, *y, *color)),
                    _ => None,
                })
                .collect()
        }
        let pipe = RenderPipeline::new(400.0, 600.0);
        let g = pipe.render(
            "<p>hi</p>",
            "p { color: rgb(0,0,0); text-shadow: 3px 4px rgb(7,8,9); }",
        );
        let ts = text_draws(&g);
        assert!(ts.len() >= 2, "shadow + main text expected: {ts:?}");
        let shadow = ts.iter().find(|(_, _, c)| c.r == 7).expect("shadow color");
        let main = ts.iter().find(|(_, _, c)| c.r == 0).expect("main color");
        // The shadow is offset from the main text by 3px / 4px.
        assert!((shadow.0 - main.0 - 3.0).abs() < 0.5, "x offset: {ts:?}");
        assert!((shadow.1 - main.1 - 4.0).abs() < 0.5, "y offset: {ts:?}");
        // No text-shadow -> a single text draw.
        let plain = pipe.render("<p>hi</p>", "p { color: rgb(0,0,0); }");
        assert_eq!(text_draws(&plain).len(), 1, "no shadow -> one draw");
    }

    #[test]
    fn html_presentational_attrs() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // img width=/height= attributes set the layout size.
        let lay = pipe.render_to_layout("<img id=\"i\" width=\"200\" height=\"100\">", "");
        let img = find_by_id(&lay, "i").expect("img");
        assert!(
            (img.dimensions.content.width - 200.0).abs() < 0.5,
            "w={}",
            img.dimensions.content.width
        );
        assert!(
            (img.dimensions.content.height - 100.0).abs() < 0.5,
            "h={}",
            img.dimensions.content.height
        );
        // A CSS rule overrides the presentational hint (lower priority).
        let lay2 = pipe.render_to_layout("<img id=\"i\" width=\"200\">", "img { width: 50px; }");
        let img2 = find_by_id(&lay2, "i").expect("img");
        assert!(
            (img2.dimensions.content.width - 50.0).abs() < 0.5,
            "CSS wins: {}",
            img2.dimensions.content.width
        );
        // bgcolor= maps to background-color (table -> a solid fill #0a141e = 10,20,30).
        let g = pipe.render("<table bgcolor=\"#0a141e\">x</table>", "");
        let bg = g.commands.iter().find_map(|c| match c {
            PaintCommand::FillRect { color, .. } => Some(*color),
            _ => None,
        });
        assert!(
            bg.map_or(false, |c| (c.r, c.g, c.b) == (10, 20, 30)),
            "bgcolor -> background: {bg:?}"
        );
    }

    #[test]
    fn css_inline_block_min_max_width() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // min-width clamps a too-small inline-block up.
        let lay = pipe.render_to_layout("<img id=\"i\" width=\"20\">", "img { min-width: 80px; }");
        let w = find_by_id(&lay, "i").expect("i").dimensions.content.width;
        assert!((w - 80.0).abs() < 0.5, "min-width clamp: {w}");
        // max-width clamps a too-large inline-block down.
        let lay2 =
            pipe.render_to_layout("<img id=\"i\" width=\"200\">", "img { max-width: 100px; }");
        let w2 = find_by_id(&lay2, "i").expect("i").dimensions.content.width;
        assert!((w2 - 100.0).abs() < 0.5, "max-width clamp: {w2}");
    }

    #[test]
    fn html_whitespace_collapsing() {
        fn all_text(node: &LayoutBox) -> String {
            let mut s = String::new();
            collect_layout_text(node, &mut s);
            s
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        let lay = pipe.render_to_layout("<p>hello     world</p>", "");
        let t = all_text(&lay);
        assert!(t.contains("hello world"), "collapsed: {t:?}");
        assert!(!t.contains("hello  "), "no double space: {t:?}");
        let lay2 = pipe.render_to_layout("<p>\n    a\n    b\n</p>", "");
        let t2 = all_text(&lay2);
        assert!(!t2.contains('\n'), "newlines collapsed: {t2:?}");
        assert!(t2.contains("a b"), "indented -> 'a b': {t2:?}");
        let lay3 = pipe.render_to_layout("<pre>a    b</pre>", "");
        assert!(
            all_text(&lay3).contains("a    b"),
            "pre preserved: {:?}",
            all_text(&lay3)
        );
    }

    #[test]
    fn html_hr_draws_visible_line() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        let lay = pipe.render_to_layout("<hr id=\"r\">", "");
        let hr = find_by_id(&lay, "r").expect("hr box");
        // 1px tall, so it renders as a thin rule rather than collapsing to nothing.
        assert!(
            (hr.dimensions.content.height - 1.0).abs() < 0.5,
            "hr height: {}",
            hr.dimensions.content.height
        );
        // Has a (gray) background so it's actually painted.
        match hr.style.get("background-color") {
            Some(CssValue::Color(c)) => assert!(c.a > 0.0, "hr bg transparent"),
            other => panic!("hr has no background color: {other:?}"),
        }
        // Spans (most of) the container width, like a real rule.
        assert!(
            hr.dimensions.content.width > 400.0,
            "hr width: {}",
            hr.dimensions.content.width
        );
    }

    #[test]
    fn html_blockquote_figure_indent() {
        fn find_by_id<'a>(node: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
            if node.node_id.as_deref() == Some(id) {
                return Some(node);
            }
            for c in &node.children {
                if let Some(b) = find_by_id(c, id) {
                    return Some(b);
                }
            }
            None
        }
        let pipe = RenderPipeline::new(800.0, 600.0);
        // blockquote: margin 16px top/bottom, 40px left/right (the quote indent).
        let lay = pipe.render_to_layout("<blockquote id=\"q\">hi</blockquote>", "");
        let q = find_by_id(&lay, "q").expect("blockquote box");
        assert!(
            (q.dimensions.margin.left - 40.0).abs() < 0.5,
            "blockquote left indent: {}",
            q.dimensions.margin.left
        );
        assert!(
            (q.dimensions.margin.right - 40.0).abs() < 0.5,
            "blockquote right indent: {}",
            q.dimensions.margin.right
        );
        assert!(
            (q.dimensions.margin.top - 16.0).abs() < 0.5,
            "blockquote top margin: {}",
            q.dimensions.margin.top
        );
        // Content is inset from the container by the left indent.
        assert!(
            q.dimensions.content.x >= 40.0,
            "blockquote content x: {}",
            q.dimensions.content.x
        );
        // <figure> gets the same indent.
        let lay2 = pipe.render_to_layout("<figure id=\"f\">x</figure>", "");
        let f = find_by_id(&lay2, "f").expect("figure box");
        assert!(
            (f.dimensions.margin.left - 40.0).abs() < 0.5,
            "figure left indent: {}",
            f.dimensions.margin.left
        );
    }

    #[test]
    fn html_document_title_extraction() {
        // Title text is found, whitespace-collapsed, and trimmed.
        let dom =
            parse_html("<html><head><title>  Hello   World  </title></head><body>x</body></html>");
        assert_eq!(document_title(&dom).as_deref(), Some("Hello World"));
        // Entities in the title are decoded (RCDATA-ish), like real chrome shows.
        let dom_ent = parse_html("<title>A &amp; B</title>");
        assert_eq!(document_title(&dom_ent).as_deref(), Some("A & B"));
        // No <title> -> None (browser falls back to the URL).
        assert_eq!(document_title(&parse_html("<body>no title</body>")), None);
        // Whitespace-only title -> None, not an empty tab label.
        assert_eq!(document_title(&parse_html("<title>   </title>")), None);
        // RenderPipeline convenience does parse+extract in one call.
        let pipe = RenderPipeline::new(800.0, 600.0);
        assert_eq!(
            pipe.page_title("<title>Tab</title>").as_deref(),
            Some("Tab")
        );
    }
}
