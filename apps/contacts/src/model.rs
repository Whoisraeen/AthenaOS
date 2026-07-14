//! Contacts data model for the AthenaOS Contacts app (Contact/Phone/Email/vCard,
//! sort/filter/groups). Moved from the former `athshell::contacts_app`; the app
//! renders a subset, so unused model surface is expected.
#![allow(dead_code, unused_imports, static_mut_refs)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Phone / email / address types ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneType {
    Mobile,
    Home,
    Work,
    Fax,
    Pager,
    Main,
    Other,
    Custom,
}

#[derive(Debug, Clone)]
pub struct Phone {
    pub kind: PhoneType,
    pub custom_label: String,
    pub number: String,
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailType {
    Home,
    Work,
    Other,
}

#[derive(Debug, Clone)]
pub struct Email {
    pub kind: EmailType,
    pub address: String,
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressType {
    Home,
    Work,
    Other,
}

#[derive(Debug, Clone)]
pub struct PostalAddress {
    pub kind: AddressType,
    pub street: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    pub country: String,
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebsiteType {
    Home,
    Work,
    Blog,
    Other,
}

#[derive(Debug, Clone)]
pub struct Website {
    pub kind: WebsiteType,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocialType {
    Twitter,
    LinkedIn,
    Facebook,
    GitHub,
    Custom,
}

#[derive(Debug, Clone)]
pub struct SocialProfile {
    pub kind: SocialType,
    pub custom_label: String,
    pub handle: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImType {
    Skype,
    Telegram,
    Discord,
    Custom,
}

#[derive(Debug, Clone)]
pub struct InstantMessage {
    pub kind: ImType,
    pub custom_label: String,
    pub handle: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationType {
    Spouse,
    Partner,
    Assistant,
    Manager,
    Child,
    Parent,
    Sibling,
    Friend,
    Custom,
}

#[derive(Debug, Clone)]
pub struct Relationship {
    pub kind: RelationType,
    pub custom_label: String,
    pub name: String,
}

// ── Contact ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Contact {
    pub id: u64,
    pub first_name: String,
    pub last_name: String,
    pub display_name: String,
    pub nickname: String,
    pub prefix: String,
    pub suffix: String,
    pub organization: String,
    pub title: String,
    pub department: String,
    pub birthday: String,
    pub anniversary: String,
    pub notes: String,
    pub photo: Vec<u8>,
    pub favorite: bool,
    pub groups: Vec<String>,
    pub phones: Vec<Phone>,
    pub emails: Vec<Email>,
    pub addresses: Vec<PostalAddress>,
    pub websites: Vec<Website>,
    pub social: Vec<SocialProfile>,
    pub ims: Vec<InstantMessage>,
    pub relationships: Vec<Relationship>,
    pub linked_ids: Vec<u64>,
    pub created_at: u64,
    pub modified_at: u64,
}

impl Contact {
    pub fn new(id: u64, first: &str, last: &str) -> Self {
        let display = if last.is_empty() {
            String::from(first)
        } else if first.is_empty() {
            String::from(last)
        } else {
            format!("{} {}", first, last)
        };
        Self {
            id,
            first_name: String::from(first),
            last_name: String::from(last),
            display_name: display,
            nickname: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            organization: String::new(),
            title: String::new(),
            department: String::new(),
            birthday: String::new(),
            anniversary: String::new(),
            notes: String::new(),
            photo: Vec::new(),
            favorite: false,
            groups: Vec::new(),
            phones: Vec::new(),
            emails: Vec::new(),
            addresses: Vec::new(),
            websites: Vec::new(),
            social: Vec::new(),
            ims: Vec::new(),
            relationships: Vec::new(),
            linked_ids: Vec::new(),
            created_at: 0,
            modified_at: 0,
        }
    }

    pub fn full_name(&self) -> String {
        let mut parts = Vec::new();
        if !self.prefix.is_empty() {
            parts.push(self.prefix.as_str());
        }
        if !self.first_name.is_empty() {
            parts.push(self.first_name.as_str());
        }
        if !self.last_name.is_empty() {
            parts.push(self.last_name.as_str());
        }
        if !self.suffix.is_empty() {
            parts.push(self.suffix.as_str());
        }
        let mut out = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(p);
        }
        out
    }

    pub fn sort_key(&self) -> String {
        let mut key = self.last_name.clone();
        key.push(' ');
        key.push_str(&self.first_name);
        key
    }

    pub fn primary_phone(&self) -> Option<&str> {
        self.phones
            .iter()
            .find(|p| p.preferred)
            .or_else(|| self.phones.first())
            .map(|p| p.number.as_str())
    }

    pub fn primary_email(&self) -> Option<&str> {
        self.emails
            .iter()
            .find(|e| e.preferred)
            .or_else(|| self.emails.first())
            .map(|e| e.address.as_str())
    }
}

// ── vCard import / export ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VCardVersion {
    V3,
    V4,
}

pub struct VCardSerializer;

impl VCardSerializer {
    pub fn export(contact: &Contact, version: VCardVersion) -> String {
        let mut out = String::new();
        out.push_str("BEGIN:VCARD\r\n");
        match version {
            VCardVersion::V3 => out.push_str("VERSION:3.0\r\n"),
            VCardVersion::V4 => out.push_str("VERSION:4.0\r\n"),
        }
        out.push_str("FN:");
        out.push_str(&contact.display_name);
        out.push_str("\r\n");

        out.push_str("N:");
        out.push_str(&contact.last_name);
        out.push(';');
        out.push_str(&contact.first_name);
        out.push_str(";;");
        out.push_str(&contact.prefix);
        out.push(';');
        out.push_str(&contact.suffix);
        out.push_str("\r\n");

        if !contact.nickname.is_empty() {
            out.push_str("NICKNAME:");
            out.push_str(&contact.nickname);
            out.push_str("\r\n");
        }
        if !contact.organization.is_empty() {
            out.push_str("ORG:");
            out.push_str(&contact.organization);
            out.push_str("\r\n");
        }
        if !contact.title.is_empty() {
            out.push_str("TITLE:");
            out.push_str(&contact.title);
            out.push_str("\r\n");
        }
        if !contact.birthday.is_empty() {
            out.push_str("BDAY:");
            out.push_str(&contact.birthday);
            out.push_str("\r\n");
        }
        if !contact.notes.is_empty() {
            out.push_str("NOTE:");
            out.push_str(&contact.notes);
            out.push_str("\r\n");
        }
        if !contact.groups.is_empty() {
            out.push_str("CATEGORIES:");
            for (i, g) in contact.groups.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(g);
            }
            out.push_str("\r\n");
        }

        for phone in &contact.phones {
            let type_str = match phone.kind {
                PhoneType::Mobile => "CELL",
                PhoneType::Home => "HOME",
                PhoneType::Work => "WORK",
                PhoneType::Fax => "FAX",
                PhoneType::Pager => "PAGER",
                PhoneType::Main => "MAIN",
                PhoneType::Other => "OTHER",
                PhoneType::Custom => phone.custom_label.as_str(),
            };
            out.push_str("TEL;TYPE=");
            out.push_str(type_str);
            if phone.preferred {
                out.push_str(";PREF=1");
            }
            out.push(':');
            out.push_str(&phone.number);
            out.push_str("\r\n");
        }

        for email in &contact.emails {
            let type_str = match email.kind {
                EmailType::Home => "HOME",
                EmailType::Work => "WORK",
                EmailType::Other => "OTHER",
            };
            out.push_str("EMAIL;TYPE=");
            out.push_str(type_str);
            if email.preferred {
                out.push_str(";PREF=1");
            }
            out.push(':');
            out.push_str(&email.address);
            out.push_str("\r\n");
        }

        for addr in &contact.addresses {
            let type_str = match addr.kind {
                AddressType::Home => "HOME",
                AddressType::Work => "WORK",
                AddressType::Other => "OTHER",
            };
            out.push_str("ADR;TYPE=");
            out.push_str(type_str);
            out.push_str(":;;");
            out.push_str(&addr.street);
            out.push(';');
            out.push_str(&addr.city);
            out.push(';');
            out.push_str(&addr.state);
            out.push(';');
            out.push_str(&addr.zip);
            out.push(';');
            out.push_str(&addr.country);
            out.push_str("\r\n");
        }

        for ws in &contact.websites {
            let type_str = match ws.kind {
                WebsiteType::Home => "HOME",
                WebsiteType::Work => "WORK",
                WebsiteType::Blog => "BLOG",
                WebsiteType::Other => "OTHER",
            };
            out.push_str("URL;TYPE=");
            out.push_str(type_str);
            out.push(':');
            out.push_str(&ws.url);
            out.push_str("\r\n");
        }

        if !contact.photo.is_empty() {
            out.push_str("PHOTO;ENCODING=b;TYPE=JPEG:");
            out.push_str("[base64data]");
            out.push_str("\r\n");
        }

        out.push_str("END:VCARD\r\n");
        out
    }

    pub fn parse(data: &str) -> Option<Contact> {
        let mut contact = Contact::new(0, "", "");
        let mut in_vcard = false;

        for line in data.lines() {
            let line = line.trim();
            if line == "BEGIN:VCARD" {
                in_vcard = true;
                continue;
            }
            if line == "END:VCARD" {
                break;
            }
            if !in_vcard {
                continue;
            }

            if let Some(val) = line.strip_prefix("FN:") {
                contact.display_name = String::from(val);
            } else if let Some(val) = line.strip_prefix("N:") {
                let parts: Vec<&str> = val.split(';').collect();
                if parts.len() >= 2 {
                    contact.last_name = String::from(parts[0]);
                    contact.first_name = String::from(parts[1]);
                }
                if parts.len() >= 4 {
                    contact.prefix = String::from(parts[3]);
                }
                if parts.len() >= 5 {
                    contact.suffix = String::from(parts[4]);
                }
            } else if let Some(val) = line.strip_prefix("NICKNAME:") {
                contact.nickname = String::from(val);
            } else if let Some(val) = line.strip_prefix("ORG:") {
                contact.organization = String::from(val);
            } else if let Some(val) = line.strip_prefix("TITLE:") {
                contact.title = String::from(val);
            } else if let Some(val) = line.strip_prefix("BDAY:") {
                contact.birthday = String::from(val);
            } else if let Some(val) = line.strip_prefix("NOTE:") {
                contact.notes = String::from(val);
            } else if let Some(val) = line.strip_prefix("CATEGORIES:") {
                contact.groups = val.split(',').map(|s| String::from(s.trim())).collect();
            } else if line.starts_with("TEL") {
                if let Some(colon) = line.find(':') {
                    let params = &line[..colon];
                    let number = String::from(&line[colon + 1..]);
                    let kind = if params.contains("CELL") {
                        PhoneType::Mobile
                    } else if params.contains("HOME") {
                        PhoneType::Home
                    } else if params.contains("WORK") {
                        PhoneType::Work
                    } else if params.contains("FAX") {
                        PhoneType::Fax
                    } else if params.contains("PAGER") {
                        PhoneType::Pager
                    } else {
                        PhoneType::Other
                    };
                    let preferred = params.contains("PREF");
                    contact.phones.push(Phone {
                        kind,
                        custom_label: String::new(),
                        number,
                        preferred,
                    });
                }
            } else if line.starts_with("EMAIL") {
                if let Some(colon) = line.find(':') {
                    let params = &line[..colon];
                    let address = String::from(&line[colon + 1..]);
                    let kind = if params.contains("HOME") {
                        EmailType::Home
                    } else if params.contains("WORK") {
                        EmailType::Work
                    } else {
                        EmailType::Other
                    };
                    let preferred = params.contains("PREF");
                    contact.emails.push(Email {
                        kind,
                        address,
                        preferred,
                    });
                }
            } else if line.starts_with("ADR") {
                if let Some(colon) = line.find(':') {
                    let params = &line[..colon];
                    let val = &line[colon + 1..];
                    let kind = if params.contains("HOME") {
                        AddressType::Home
                    } else if params.contains("WORK") {
                        AddressType::Work
                    } else {
                        AddressType::Other
                    };
                    let parts: Vec<&str> = val.split(';').collect();
                    contact.addresses.push(PostalAddress {
                        kind,
                        street: String::from(parts.get(2).copied().unwrap_or("")),
                        city: String::from(parts.get(3).copied().unwrap_or("")),
                        state: String::from(parts.get(4).copied().unwrap_or("")),
                        zip: String::from(parts.get(5).copied().unwrap_or("")),
                        country: String::from(parts.get(6).copied().unwrap_or("")),
                        preferred: false,
                    });
                }
            } else if line.starts_with("URL") {
                if let Some(colon) = line.find(':') {
                    let params = &line[..colon];
                    let url = String::from(&line[colon + 1..]);
                    let kind = if params.contains("HOME") {
                        WebsiteType::Home
                    } else if params.contains("WORK") {
                        WebsiteType::Work
                    } else if params.contains("BLOG") {
                        WebsiteType::Blog
                    } else {
                        WebsiteType::Other
                    };
                    contact.websites.push(Website { kind, url });
                }
            }
        }

        if contact.display_name.is_empty() && contact.first_name.is_empty() {
            None
        } else {
            Some(contact)
        }
    }

    pub fn export_multiple(contacts: &[Contact], version: VCardVersion) -> String {
        let mut out = String::new();
        for c in contacts {
            out.push_str(&Self::export(c, version));
        }
        out
    }
}

// ── CardDAV client ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardDavMethod {
    Propfind,
    Report,
    Put,
    Delete,
    Get,
}

#[derive(Debug, Clone)]
pub struct CardDavServer {
    pub url: String,
    pub username: String,
    pub password: String,
    pub address_book_path: String,
    pub sync_token: String,
    pub last_sync: u64,
}

impl CardDavServer {
    pub fn new(url: &str, user: &str, pass: &str) -> Self {
        Self {
            url: String::from(url),
            username: String::from(user),
            password: String::from(pass),
            address_book_path: String::new(),
            sync_token: String::new(),
            last_sync: 0,
        }
    }

    pub fn discover_address_book(&mut self) -> Result<(), &'static str> {
        self.address_book_path = format!("{}/.well-known/carddav", self.url);
        Ok(())
    }

    pub fn build_propfind_request(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n\
             <d:propfind xmlns:d=\"DAV:\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\">\r\n\
             <d:prop><d:getetag/><card:address-data/></d:prop>\r\n\
             </d:propfind>"
        )
    }

    pub fn build_report_request(&self, sync_token: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n\
             <d:sync-collection xmlns:d=\"DAV:\">\r\n\
             <d:sync-token>{}</d:sync-token>\r\n\
             <d:prop><d:getetag/></d:prop>\r\n\
             </d:sync-collection>",
            sync_token
        )
    }

    pub fn build_put_request(&self, vcard: &str) -> String {
        String::from(vcard)
    }

    pub fn update_sync_token(&mut self, token: &str, timestamp: u64) {
        self.sync_token = String::from(token);
        self.last_sync = timestamp;
    }
}

// ── Contact group ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContactGroup {
    pub id: u64,
    pub name: String,
    pub member_ids: Vec<u64>,
    pub color: u32,
}

impl ContactGroup {
    pub fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            member_ids: Vec::new(),
            color: 0xFF_4E_9C_FF,
        }
    }

    pub fn add_member(&mut self, contact_id: u64) {
        if !self.member_ids.contains(&contact_id) {
            self.member_ids.push(contact_id);
        }
    }

    pub fn remove_member(&mut self, contact_id: u64) {
        self.member_ids.retain(|&id| id != contact_id);
    }

    pub fn has_member(&self, contact_id: u64) -> bool {
        self.member_ids.contains(&contact_id)
    }

    pub fn member_count(&self) -> usize {
        self.member_ids.len()
    }
}

// ── Duplicate detection ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateMatch {
    ByName,
    ByPhone,
    ByEmail,
    ByNameAndPhone,
    ByNameAndEmail,
}

#[derive(Debug, Clone)]
pub struct DuplicatePair {
    pub id_a: u64,
    pub id_b: u64,
    pub match_type: DuplicateMatch,
    pub confidence: u8,
}

pub struct DuplicateDetector;

impl DuplicateDetector {
    pub fn find_duplicates(contacts: &[Contact]) -> Vec<DuplicatePair> {
        let mut pairs = Vec::new();
        for i in 0..contacts.len() {
            for j in (i + 1)..contacts.len() {
                let a = &contacts[i];
                let b = &contacts[j];

                if !a.first_name.is_empty()
                    && a.first_name == b.first_name
                    && !a.last_name.is_empty()
                    && a.last_name == b.last_name
                {
                    let has_phone = Self::phones_overlap(a, b);
                    let has_email = Self::emails_overlap(a, b);
                    let (match_type, confidence) = if has_phone && has_email {
                        (DuplicateMatch::ByNameAndEmail, 99)
                    } else if has_phone {
                        (DuplicateMatch::ByNameAndPhone, 90)
                    } else if has_email {
                        (DuplicateMatch::ByNameAndEmail, 85)
                    } else {
                        (DuplicateMatch::ByName, 60)
                    };
                    pairs.push(DuplicatePair {
                        id_a: a.id,
                        id_b: b.id,
                        match_type,
                        confidence,
                    });
                    continue;
                }

                if Self::phones_overlap(a, b) {
                    pairs.push(DuplicatePair {
                        id_a: a.id,
                        id_b: b.id,
                        match_type: DuplicateMatch::ByPhone,
                        confidence: 70,
                    });
                } else if Self::emails_overlap(a, b) {
                    pairs.push(DuplicatePair {
                        id_a: a.id,
                        id_b: b.id,
                        match_type: DuplicateMatch::ByEmail,
                        confidence: 75,
                    });
                }
            }
        }
        pairs
    }

    fn phones_overlap(a: &Contact, b: &Contact) -> bool {
        for pa in &a.phones {
            for pb in &b.phones {
                let na = pa.number.replace(['-', ' ', '(', ')'], "");
                let nb = pb.number.replace(['-', ' ', '(', ')'], "");
                if !na.is_empty() && na == nb {
                    return true;
                }
            }
        }
        false
    }

    fn emails_overlap(a: &Contact, b: &Contact) -> bool {
        for ea in &a.emails {
            for eb in &b.emails {
                if !ea.address.is_empty() && ea.address == eb.address {
                    return true;
                }
            }
        }
        false
    }

    pub fn merge(primary: &mut Contact, secondary: &Contact) {
        if primary.nickname.is_empty() && !secondary.nickname.is_empty() {
            primary.nickname = secondary.nickname.clone();
        }
        if primary.organization.is_empty() && !secondary.organization.is_empty() {
            primary.organization = secondary.organization.clone();
        }
        if primary.title.is_empty() && !secondary.title.is_empty() {
            primary.title = secondary.title.clone();
        }
        if primary.department.is_empty() && !secondary.department.is_empty() {
            primary.department = secondary.department.clone();
        }
        if primary.birthday.is_empty() && !secondary.birthday.is_empty() {
            primary.birthday = secondary.birthday.clone();
        }
        if primary.notes.is_empty() && !secondary.notes.is_empty() {
            primary.notes = secondary.notes.clone();
        }
        for phone in &secondary.phones {
            let dup = primary.phones.iter().any(|p| p.number == phone.number);
            if !dup {
                primary.phones.push(phone.clone());
            }
        }
        for email in &secondary.emails {
            let dup = primary.emails.iter().any(|e| e.address == email.address);
            if !dup {
                primary.emails.push(email.clone());
            }
        }
        for addr in &secondary.addresses {
            primary.addresses.push(addr.clone());
        }
        for ws in &secondary.websites {
            let dup = primary.websites.iter().any(|w| w.url == ws.url);
            if !dup {
                primary.websites.push(ws.clone());
            }
        }
        for sp in &secondary.social {
            let dup = primary
                .social
                .iter()
                .any(|s| s.handle == sp.handle && s.kind == sp.kind);
            if !dup {
                primary.social.push(sp.clone());
            }
        }
        for im in &secondary.ims {
            let dup = primary
                .ims
                .iter()
                .any(|i| i.handle == im.handle && i.kind == im.kind);
            if !dup {
                primary.ims.push(im.clone());
            }
        }
        primary.linked_ids.push(secondary.id);
    }
}

// ── Quick actions ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    Call,
    Text,
    Email,
    MapAddress,
    VideoCall,
}

#[derive(Debug, Clone)]
pub struct QuickActionRequest {
    pub action: QuickAction,
    pub contact_id: u64,
    pub target: String,
}

// ── Import / export formats ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    VCard,
    Csv,
    Ldif,
    WindowsContacts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    VCard3,
    VCard4,
    Csv,
    Ldif,
}

pub struct CsvExporter;

impl CsvExporter {
    pub fn export(contacts: &[Contact]) -> String {
        let mut out =
            String::from("First Name,Last Name,Display Name,Organization,Title,Phone,Email\r\n");
        for c in contacts {
            out.push_str(&c.first_name);
            out.push(',');
            out.push_str(&c.last_name);
            out.push(',');
            out.push_str(&c.display_name);
            out.push(',');
            out.push_str(&c.organization);
            out.push(',');
            out.push_str(&c.title);
            out.push(',');
            out.push_str(c.primary_phone().unwrap_or(""));
            out.push(',');
            out.push_str(c.primary_email().unwrap_or(""));
            out.push_str("\r\n");
        }
        out
    }

    pub fn parse(data: &str) -> Vec<Contact> {
        let mut contacts = Vec::new();
        let mut next_id = 1u64;
        let mut lines = data.lines();
        let _header = lines.next();
        for line in lines {
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() >= 2 {
                let first = cols.first().copied().unwrap_or("");
                let last = cols.get(1).copied().unwrap_or("");
                let mut c = Contact::new(next_id, first, last);
                next_id += 1;
                if let Some(org) = cols.get(3) {
                    c.organization = String::from(*org);
                }
                if let Some(title) = cols.get(4) {
                    c.title = String::from(*title);
                }
                if let Some(phone) = cols.get(5) {
                    if !phone.is_empty() {
                        c.phones.push(Phone {
                            kind: PhoneType::Mobile,
                            custom_label: String::new(),
                            number: String::from(*phone),
                            preferred: true,
                        });
                    }
                }
                if let Some(email) = cols.get(6) {
                    if !email.is_empty() {
                        c.emails.push(Email {
                            kind: EmailType::Home,
                            address: String::from(*email),
                            preferred: true,
                        });
                    }
                }
                contacts.push(c);
            }
        }
        contacts
    }
}

// ── Sort / filter ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactSortBy {
    FirstName,
    LastName,
    DisplayName,
    Organization,
    RecentlyAdded,
    RecentlyModified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactFilter {
    All,
    Favorites,
    Group,
    RecentlyContacted,
    RecentlyAdded,
    RecentlyViewed,
}

// ── Alphabetical index ──────────────────────────────────────────────────

pub struct AlphaIndex {
    pub letters: Vec<char>,
    pub selected: Option<char>,
}

impl AlphaIndex {
    pub fn new() -> Self {
        Self {
            letters: ('A'..='Z').collect(),
            selected: None,
        }
    }

    pub fn select(&mut self, letter: char) {
        self.selected = Some(letter);
    }

    pub fn clear(&mut self) {
        self.selected = None;
    }

    pub fn filter_contacts<'a>(&self, contacts: &'a [Contact]) -> Vec<&'a Contact> {
        match self.selected {
            Some(letter) => contacts
                .iter()
                .filter(|c| c.last_name.starts_with(letter) || c.first_name.starts_with(letter))
                .collect(),
            None => contacts.iter().collect(),
        }
    }
}

// ── Recent contacts ─────────────────────────────────────────────────────

pub struct RecentContacts {
    pub most_contacted: Vec<(u64, u32)>,
    pub recently_added: Vec<u64>,
    pub recently_viewed: Vec<u64>,
    pub max_entries: usize,
}

impl RecentContacts {
    pub fn new(max_entries: usize) -> Self {
        Self {
            most_contacted: Vec::new(),
            recently_added: Vec::new(),
            recently_viewed: Vec::new(),
            max_entries,
        }
    }

    pub fn record_contact(&mut self, id: u64) {
        if let Some(entry) = self.most_contacted.iter_mut().find(|(eid, _)| *eid == id) {
            entry.1 += 1;
        } else {
            self.most_contacted.push((id, 1));
        }
        self.most_contacted.sort_by(|a, b| b.1.cmp(&a.1));
        if self.most_contacted.len() > self.max_entries {
            self.most_contacted.truncate(self.max_entries);
        }
    }

    pub fn record_view(&mut self, id: u64) {
        self.recently_viewed.retain(|&vid| vid != id);
        self.recently_viewed.insert(0, id);
        if self.recently_viewed.len() > self.max_entries {
            self.recently_viewed.truncate(self.max_entries);
        }
    }

    pub fn record_added(&mut self, id: u64) {
        self.recently_added.insert(0, id);
        if self.recently_added.len() > self.max_entries {
            self.recently_added.truncate(self.max_entries);
        }
    }
}

// ── Contacts app ────────────────────────────────────────────────────────

pub struct ContactsApp {
    pub contacts: Vec<Contact>,
    pub groups: Vec<ContactGroup>,
    pub next_contact_id: u64,
    pub next_group_id: u64,
    pub sort_by: ContactSortBy,
    pub filter: ContactFilter,
    pub filter_group_id: Option<u64>,
    pub search_query: String,
    pub alpha_index: AlphaIndex,
    pub recent: RecentContacts,
    pub carddav: Option<CardDavServer>,
    pub selected_contact: Option<u64>,
}

impl ContactsApp {
    pub fn new() -> Self {
        Self {
            contacts: Vec::new(),
            groups: Vec::new(),
            next_contact_id: 1,
            next_group_id: 1,
            sort_by: ContactSortBy::LastName,
            filter: ContactFilter::All,
            filter_group_id: None,
            search_query: String::new(),
            alpha_index: AlphaIndex::new(),
            recent: RecentContacts::new(20),
            carddav: None,
            selected_contact: None,
        }
    }

    pub fn add_contact(&mut self, first: &str, last: &str) -> u64 {
        let id = self.next_contact_id;
        self.next_contact_id += 1;
        let contact = Contact::new(id, first, last);
        self.contacts.push(contact);
        self.recent.record_added(id);
        id
    }

    pub fn remove_contact(&mut self, id: u64) {
        self.contacts.retain(|c| c.id != id);
        for group in &mut self.groups {
            group.remove_member(id);
        }
    }

    pub fn get_contact(&self, id: u64) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.id == id)
    }

    pub fn get_contact_mut(&mut self, id: u64) -> Option<&mut Contact> {
        self.contacts.iter_mut().find(|c| c.id == id)
    }

    pub fn create_group(&mut self, name: &str) -> u64 {
        let id = self.next_group_id;
        self.next_group_id += 1;
        self.groups.push(ContactGroup::new(id, name));
        id
    }

    pub fn rename_group(&mut self, id: u64, name: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.name = String::from(name);
        }
    }

    pub fn delete_group(&mut self, id: u64) {
        self.groups.retain(|g| g.id != id);
    }

    pub fn add_to_group(&mut self, group_id: u64, contact_id: u64) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) {
            g.add_member(contact_id);
        }
        if let Some(c) = self.contacts.iter_mut().find(|c| c.id == contact_id) {
            if let Some(g) = self.groups.iter().find(|g| g.id == group_id) {
                if !c.groups.contains(&g.name) {
                    c.groups.push(g.name.clone());
                }
            }
        }
    }

    pub fn remove_from_group(&mut self, group_id: u64, contact_id: u64) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == group_id) {
            g.remove_member(contact_id);
        }
    }

    pub fn search(&self, query: &str) -> Vec<&Contact> {
        let q = query;
        self.contacts
            .iter()
            .filter(|c| {
                c.first_name.contains(q)
                    || c.last_name.contains(q)
                    || c.display_name.contains(q)
                    || c.organization.contains(q)
                    || c.phones.iter().any(|p| p.number.contains(q))
                    || c.emails.iter().any(|e| e.address.contains(q))
            })
            .collect()
    }

    pub fn sorted_contacts(&self) -> Vec<&Contact> {
        let mut list: Vec<&Contact> = match self.filter {
            ContactFilter::All => self.contacts.iter().collect(),
            ContactFilter::Favorites => self.contacts.iter().filter(|c| c.favorite).collect(),
            ContactFilter::Group => {
                if let Some(gid) = self.filter_group_id {
                    if let Some(group) = self.groups.iter().find(|g| g.id == gid) {
                        self.contacts
                            .iter()
                            .filter(|c| group.has_member(c.id))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    self.contacts.iter().collect()
                }
            }
            _ => self.contacts.iter().collect(),
        };

        if !self.search_query.is_empty() {
            let q = &self.search_query;
            list.retain(|c| {
                c.first_name.contains(q.as_str())
                    || c.last_name.contains(q.as_str())
                    || c.display_name.contains(q.as_str())
                    || c.organization.contains(q.as_str())
            });
        }

        match self.sort_by {
            ContactSortBy::FirstName => list.sort_by(|a, b| a.first_name.cmp(&b.first_name)),
            ContactSortBy::LastName => list.sort_by(|a, b| a.last_name.cmp(&b.last_name)),
            ContactSortBy::DisplayName => list.sort_by(|a, b| a.display_name.cmp(&b.display_name)),
            ContactSortBy::Organization => list.sort_by(|a, b| a.organization.cmp(&b.organization)),
            ContactSortBy::RecentlyAdded => list.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
            ContactSortBy::RecentlyModified => {
                list.sort_by(|a, b| b.modified_at.cmp(&a.modified_at))
            }
        }
        list
    }

    pub fn find_duplicates(&self) -> Vec<DuplicatePair> {
        DuplicateDetector::find_duplicates(&self.contacts)
    }

    pub fn merge_contacts(&mut self, primary_id: u64, secondary_id: u64) {
        let secondary = self.contacts.iter().find(|c| c.id == secondary_id).cloned();
        if let Some(sec) = secondary {
            if let Some(primary) = self.contacts.iter_mut().find(|c| c.id == primary_id) {
                DuplicateDetector::merge(primary, &sec);
            }
            self.contacts.retain(|c| c.id != secondary_id);
        }
    }

    pub fn link_contacts(&mut self, id_a: u64, id_b: u64) {
        if let Some(a) = self.contacts.iter_mut().find(|c| c.id == id_a) {
            if !a.linked_ids.contains(&id_b) {
                a.linked_ids.push(id_b);
            }
        }
        if let Some(b) = self.contacts.iter_mut().find(|c| c.id == id_b) {
            if !b.linked_ids.contains(&id_a) {
                b.linked_ids.push(id_a);
            }
        }
    }

    pub fn unlink_contacts(&mut self, id_a: u64, id_b: u64) {
        if let Some(a) = self.contacts.iter_mut().find(|c| c.id == id_a) {
            a.linked_ids.retain(|&id| id != id_b);
        }
        if let Some(b) = self.contacts.iter_mut().find(|c| c.id == id_b) {
            b.linked_ids.retain(|&id| id != id_a);
        }
    }

    pub fn import_vcard(&mut self, data: &str) {
        let blocks: Vec<&str> = data.split("END:VCARD").collect();
        for block in blocks {
            let full = format!("{}END:VCARD", block.trim());
            if let Some(mut contact) = VCardSerializer::parse(&full) {
                contact.id = self.next_contact_id;
                self.next_contact_id += 1;
                self.contacts.push(contact);
            }
        }
    }

    pub fn export_vcard(&self, version: VCardVersion) -> String {
        VCardSerializer::export_multiple(&self.contacts, version)
    }

    pub fn import_csv(&mut self, data: &str) {
        let imported = CsvExporter::parse(data);
        for mut c in imported {
            c.id = self.next_contact_id;
            self.next_contact_id += 1;
            self.contacts.push(c);
        }
    }

    pub fn export_csv(&self) -> String {
        CsvExporter::export(&self.contacts)
    }

    pub fn setup_carddav(&mut self, url: &str, user: &str, pass: &str) {
        let mut server = CardDavServer::new(url, user, pass);
        let _ = server.discover_address_book();
        self.carddav = Some(server);
    }

    pub fn quick_action(&self, contact_id: u64, action: QuickAction) -> Option<QuickActionRequest> {
        let contact = self.get_contact(contact_id)?;
        let target = match action {
            QuickAction::Call | QuickAction::Text | QuickAction::VideoCall => {
                String::from(contact.primary_phone()?)
            }
            QuickAction::Email => String::from(contact.primary_email()?),
            QuickAction::MapAddress => {
                let addr = contact.addresses.first()?;
                format!(
                    "{}, {}, {} {}",
                    addr.street, addr.city, addr.state, addr.zip
                )
            }
        };
        Some(QuickActionRequest {
            action,
            contact_id,
            target,
        })
    }

    pub fn total_contacts(&self) -> usize {
        self.contacts.len()
    }

    pub fn favorites_count(&self) -> usize {
        self.contacts.iter().filter(|c| c.favorite).count()
    }
}

// ── Global instance ─────────────────────────────────────────────────────

static INITIALIZED: AtomicBool = AtomicBool::new(false);

static mut CONTACTS_APP_INSTANCE: Option<ContactsApp> = None;

pub fn init() {
    if INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            CONTACTS_APP_INSTANCE = Some(ContactsApp::new());
        }
    }
}

pub fn contacts() -> &'static mut ContactsApp {
    unsafe {
        CONTACTS_APP_INSTANCE
            .as_mut()
            .expect("CONTACTS_APP not initialized; call init() first")
    }
}
