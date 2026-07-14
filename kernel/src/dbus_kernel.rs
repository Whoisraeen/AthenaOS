//! Kernel-side D-Bus message bus for RaeenOS.
//!
//! Implements the D-Bus wire protocol and bus semantics inside the kernel for
//! minimal-overhead IPC: message types, header fields, type system, marshalling,
//! name ownership, signal match rules, introspection, standard interfaces, bus
//! policy, service activation, credential/FD passing, and connection management.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ───────────────────────────────────────────────────────────────────────────────
// 1. Bus Types
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType {
    System,
    Session,
    Activation,
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Message Types & Flags
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    MethodCall = 1,
    MethodReturn = 2,
    Error = 3,
    Signal = 4,
}

impl MessageType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::MethodCall),
            2 => Some(Self::MethodReturn),
            3 => Some(Self::Error),
            4 => Some(Self::Signal),
            _ => None,
        }
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MessageFlags: u8 {
        const NO_REPLY_EXPECTED              = 0x01;
        const NO_AUTO_START                  = 0x02;
        const ALLOW_INTERACTIVE_AUTHORIZATION = 0x04;
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Header Fields
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HeaderFieldCode {
    Path = 1,
    Interface = 2,
    Member = 3,
    ErrorName = 4,
    ReplySerial = 5,
    Destination = 6,
    Sender = 7,
    Signature = 8,
    UnixFds = 9,
}

#[derive(Debug, Clone)]
pub enum HeaderFieldValue {
    ObjectPath(String),
    InterfaceName(String),
    MemberName(String),
    ErrorName(String),
    ReplySerial(u32),
    BusName(String),
    Signature(String),
    UnixFdCount(u32),
}

#[derive(Debug, Clone)]
pub struct HeaderField {
    pub code: HeaderFieldCode,
    pub value: HeaderFieldValue,
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. Endianness & Message Header
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    Little,
    Big,
}

impl Endianness {
    pub fn marker(&self) -> u8 {
        match self {
            Self::Little => b'l',
            Self::Big => b'B',
        }
    }

    pub fn from_marker(m: u8) -> Option<Self> {
        match m {
            b'l' => Some(Self::Little),
            b'B' => Some(Self::Big),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessageHeader {
    pub endianness: Endianness,
    pub msg_type: MessageType,
    pub flags: MessageFlags,
    pub version: u8,
    pub body_length: u32,
    pub serial: u32,
    pub fields: Vec<HeaderField>,
}

impl MessageHeader {
    pub fn new(msg_type: MessageType, serial: u32) -> Self {
        Self {
            endianness: Endianness::Little,
            msg_type,
            flags: MessageFlags::empty(),
            version: 1,
            body_length: 0,
            serial,
            fields: Vec::new(),
        }
    }

    pub fn set_path(&mut self, path: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Path,
            value: HeaderFieldValue::ObjectPath(String::from(path)),
        });
    }

    pub fn set_interface(&mut self, iface: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Interface,
            value: HeaderFieldValue::InterfaceName(String::from(iface)),
        });
    }

    pub fn set_member(&mut self, member: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Member,
            value: HeaderFieldValue::MemberName(String::from(member)),
        });
    }

    pub fn set_destination(&mut self, dest: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Destination,
            value: HeaderFieldValue::BusName(String::from(dest)),
        });
    }

    pub fn set_sender(&mut self, sender: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Sender,
            value: HeaderFieldValue::BusName(String::from(sender)),
        });
    }

    pub fn set_error_name(&mut self, name: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::ErrorName,
            value: HeaderFieldValue::ErrorName(String::from(name)),
        });
    }

    pub fn set_reply_serial(&mut self, s: u32) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::ReplySerial,
            value: HeaderFieldValue::ReplySerial(s),
        });
    }

    pub fn set_signature(&mut self, sig: &str) {
        self.fields.push(HeaderField {
            code: HeaderFieldCode::Signature,
            value: HeaderFieldValue::Signature(String::from(sig)),
        });
    }

    pub fn get_field(&self, code: HeaderFieldCode) -> Option<&HeaderFieldValue> {
        self.fields
            .iter()
            .find(|f| f.code == code)
            .map(|f| &f.value)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. D-Bus Type System
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DbusType {
    Byte(u8),
    Boolean(bool),
    Int16(i16),
    Uint16(u16),
    Int32(i32),
    Uint32(u32),
    Int64(i64),
    Uint64(u64),
    Double(f64),
    String(String),
    ObjectPath(String),
    Signature(String),
    Array(Vec<DbusType>),
    Struct(Vec<DbusType>),
    Variant(Box<DbusType>),
    DictEntry(Box<DbusType>, Box<DbusType>),
    UnixFd(u32),
}

impl DbusType {
    pub fn type_code(&self) -> u8 {
        match self {
            Self::Byte(_) => b'y',
            Self::Boolean(_) => b'b',
            Self::Int16(_) => b'n',
            Self::Uint16(_) => b'q',
            Self::Int32(_) => b'i',
            Self::Uint32(_) => b'u',
            Self::Int64(_) => b'x',
            Self::Uint64(_) => b't',
            Self::Double(_) => b'd',
            Self::String(_) => b's',
            Self::ObjectPath(_) => b'o',
            Self::Signature(_) => b'g',
            Self::Array(_) => b'a',
            Self::Struct(_) => b'r',
            Self::Variant(_) => b'v',
            Self::DictEntry(..) => b'e',
            Self::UnixFd(_) => b'h',
        }
    }

    pub fn alignment(&self) -> usize {
        match self {
            Self::Byte(_) => 1,
            Self::Boolean(_) => 4,
            Self::Int16(_) => 2,
            Self::Uint16(_) => 2,
            Self::Int32(_) => 4,
            Self::Uint32(_) => 4,
            Self::Int64(_) => 8,
            Self::Uint64(_) => 8,
            Self::Double(_) => 8,
            Self::String(_) => 4,
            Self::ObjectPath(_) => 4,
            Self::Signature(_) => 1,
            Self::Array(_) => 4,
            Self::Struct(_) => 8,
            Self::Variant(_) => 1,
            Self::DictEntry(..) => 8,
            Self::UnixFd(_) => 4,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. Marshalling / Unmarshalling
// ───────────────────────────────────────────────────────────────────────────────

pub struct MarshalBuffer {
    pub data: Vec<u8>,
    pub endianness: Endianness,
    pub nesting: u32,
}

const MAX_CONTAINER_NESTING: u32 = 64;

impl MarshalBuffer {
    pub fn new(endianness: Endianness) -> Self {
        Self {
            data: Vec::new(),
            endianness,
            nesting: 0,
        }
    }

    fn align_to(&mut self, alignment: usize) {
        while self.data.len() % alignment != 0 {
            self.data.push(0);
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.data.push(v);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.align_to(2);
        let bytes = match self.endianness {
            Endianness::Little => v.to_le_bytes(),
            Endianness::Big => v.to_be_bytes(),
        };
        self.data.extend_from_slice(&bytes);
    }

    pub fn write_u32(&mut self, v: u32) {
        self.align_to(4);
        let bytes = match self.endianness {
            Endianness::Little => v.to_le_bytes(),
            Endianness::Big => v.to_be_bytes(),
        };
        self.data.extend_from_slice(&bytes);
    }

    pub fn write_u64(&mut self, v: u64) {
        self.align_to(8);
        let bytes = match self.endianness {
            Endianness::Little => v.to_le_bytes(),
            Endianness::Big => v.to_be_bytes(),
        };
        self.data.extend_from_slice(&bytes);
    }

    pub fn write_i16(&mut self, v: i16) {
        self.write_u16(v as u16);
    }
    pub fn write_i32(&mut self, v: i32) {
        self.write_u32(v as u32);
    }
    pub fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }

    pub fn write_f64(&mut self, v: f64) {
        self.write_u64(v.to_bits());
    }

    pub fn write_string(&mut self, s: &str) {
        self.write_u32(s.len() as u32);
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
    }

    pub fn write_signature(&mut self, s: &str) {
        self.write_u8(s.len() as u8);
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
    }

    pub fn write_boolean(&mut self, v: bool) {
        self.write_u32(if v { 1 } else { 0 });
    }

    pub fn marshal_value(&mut self, value: &DbusType) -> Result<(), &'static str> {
        match value {
            DbusType::Byte(v) => {
                self.write_u8(*v);
                Ok(())
            }
            DbusType::Boolean(v) => {
                self.write_boolean(*v);
                Ok(())
            }
            DbusType::Int16(v) => {
                self.write_i16(*v);
                Ok(())
            }
            DbusType::Uint16(v) => {
                self.write_u16(*v);
                Ok(())
            }
            DbusType::Int32(v) => {
                self.write_i32(*v);
                Ok(())
            }
            DbusType::Uint32(v) => {
                self.write_u32(*v);
                Ok(())
            }
            DbusType::Int64(v) => {
                self.write_i64(*v);
                Ok(())
            }
            DbusType::Uint64(v) => {
                self.write_u64(*v);
                Ok(())
            }
            DbusType::Double(v) => {
                self.write_f64(*v);
                Ok(())
            }
            DbusType::String(s) => {
                self.write_string(s);
                Ok(())
            }
            DbusType::ObjectPath(s) => {
                self.write_string(s);
                Ok(())
            }
            DbusType::Signature(s) => {
                self.write_signature(s);
                Ok(())
            }
            DbusType::UnixFd(v) => {
                self.write_u32(*v);
                Ok(())
            }
            DbusType::Array(items) => {
                if self.nesting >= MAX_CONTAINER_NESTING {
                    return Err("max container nesting exceeded");
                }
                self.nesting += 1;
                let len_pos = self.data.len();
                self.write_u32(0);
                let start = self.data.len();
                for item in items {
                    self.marshal_value(item)?;
                }
                let array_len = (self.data.len() - start) as u32;
                let bytes = match self.endianness {
                    Endianness::Little => array_len.to_le_bytes(),
                    Endianness::Big => array_len.to_be_bytes(),
                };
                self.data[len_pos..len_pos + 4].copy_from_slice(&bytes);
                self.nesting -= 1;
                Ok(())
            }
            DbusType::Struct(fields) => {
                if self.nesting >= MAX_CONTAINER_NESTING {
                    return Err("max container nesting exceeded");
                }
                self.nesting += 1;
                self.align_to(8);
                for f in fields {
                    self.marshal_value(f)?;
                }
                self.nesting -= 1;
                Ok(())
            }
            DbusType::Variant(inner) => {
                let sig = alloc::string::String::from(
                    core::str::from_utf8(&[inner.type_code()]).unwrap_or("?"),
                );
                self.write_signature(&sig);
                self.marshal_value(inner)
            }
            DbusType::DictEntry(k, v) => {
                self.align_to(8);
                self.marshal_value(k)?;
                self.marshal_value(v)
            }
        }
    }
}

pub struct UnmarshalBuffer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    pub endianness: Endianness,
}

impl<'a> UnmarshalBuffer<'a> {
    pub fn new(data: &'a [u8], endianness: Endianness) -> Self {
        Self {
            data,
            pos: 0,
            endianness,
        }
    }

    fn align_to(&mut self, alignment: usize) {
        while self.pos % alignment != 0 {
            self.pos += 1;
        }
    }

    pub fn read_u8(&mut self) -> Result<u8, &'static str> {
        if self.pos >= self.data.len() {
            return Err("underflow");
        }
        let v = self.data[self.pos];
        self.pos += 1;
        Ok(v)
    }

    pub fn read_u16(&mut self) -> Result<u16, &'static str> {
        self.align_to(2);
        if self.pos + 2 > self.data.len() {
            return Err("underflow");
        }
        let bytes: [u8; 2] = [self.data[self.pos], self.data[self.pos + 1]];
        self.pos += 2;
        Ok(match self.endianness {
            Endianness::Little => u16::from_le_bytes(bytes),
            Endianness::Big => u16::from_be_bytes(bytes),
        })
    }

    pub fn read_u32(&mut self) -> Result<u32, &'static str> {
        self.align_to(4);
        if self.pos + 4 > self.data.len() {
            return Err("underflow");
        }
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + 4]);
        self.pos += 4;
        Ok(match self.endianness {
            Endianness::Little => u32::from_le_bytes(bytes),
            Endianness::Big => u32::from_be_bytes(bytes),
        })
    }

    pub fn read_u64(&mut self) -> Result<u64, &'static str> {
        self.align_to(8);
        if self.pos + 8 > self.data.len() {
            return Err("underflow");
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + 8]);
        self.pos += 8;
        Ok(match self.endianness {
            Endianness::Little => u64::from_le_bytes(bytes),
            Endianness::Big => u64::from_be_bytes(bytes),
        })
    }

    pub fn read_string(&mut self) -> Result<String, &'static str> {
        let len = self.read_u32()? as usize;
        if self.pos + len >= self.data.len() {
            return Err("underflow");
        }
        let s = core::str::from_utf8(&self.data[self.pos..self.pos + len])
            .map_err(|_| "invalid utf-8")?;
        self.pos += len + 1; // +1 for NUL
        Ok(String::from(s))
    }

    pub fn read_boolean(&mut self) -> Result<bool, &'static str> {
        let v = self.read_u32()?;
        Ok(v != 0)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. D-Bus Message
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DbusMessage {
    pub header: MessageHeader,
    pub body: Vec<DbusType>,
    pub fds: Vec<u32>,
}

impl DbusMessage {
    pub fn method_call(serial: u32, dest: &str, path: &str, iface: &str, member: &str) -> Self {
        let mut hdr = MessageHeader::new(MessageType::MethodCall, serial);
        hdr.set_destination(dest);
        hdr.set_path(path);
        hdr.set_interface(iface);
        hdr.set_member(member);
        Self {
            header: hdr,
            body: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub fn method_return(serial: u32, reply_serial: u32) -> Self {
        let mut hdr = MessageHeader::new(MessageType::MethodReturn, serial);
        hdr.set_reply_serial(reply_serial);
        Self {
            header: hdr,
            body: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub fn error(serial: u32, reply_serial: u32, name: &str) -> Self {
        let mut hdr = MessageHeader::new(MessageType::Error, serial);
        hdr.set_reply_serial(reply_serial);
        hdr.set_error_name(name);
        Self {
            header: hdr,
            body: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub fn signal(serial: u32, path: &str, iface: &str, member: &str) -> Self {
        let mut hdr = MessageHeader::new(MessageType::Signal, serial);
        hdr.set_path(path);
        hdr.set_interface(iface);
        hdr.set_member(member);
        Self {
            header: hdr,
            body: Vec::new(),
            fds: Vec::new(),
        }
    }

    pub fn add_arg(&mut self, val: DbusType) {
        self.body.push(val);
    }

    pub fn attach_fd(&mut self, fd: u32) {
        self.fds.push(fd);
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 8. Name Ownership
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameRequestResult {
    PrimaryOwner,
    InQueue,
    Exists,
    AlreadyOwner,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct NameRequestFlags: u32 {
        const ALLOW_REPLACEMENT = 0x01;
        const REPLACE_EXISTING  = 0x02;
        const DO_NOT_QUEUE      = 0x04;
    }
}

#[derive(Debug, Clone)]
pub struct NameOwner {
    pub unique_name: String,
    pub flags: NameRequestFlags,
}

pub struct NameRegistry {
    owners: BTreeMap<String, NameOwner>,
    queue: BTreeMap<String, Vec<NameOwner>>,
}

impl NameRegistry {
    pub fn new() -> Self {
        Self {
            owners: BTreeMap::new(),
            queue: BTreeMap::new(),
        }
    }

    pub fn request_name(
        &mut self,
        well_known: &str,
        unique: &str,
        flags: NameRequestFlags,
    ) -> NameRequestResult {
        if let Some(existing) = self.owners.get(well_known) {
            if existing.unique_name == unique {
                return NameRequestResult::AlreadyOwner;
            }
            if flags.contains(NameRequestFlags::REPLACE_EXISTING)
                && existing.flags.contains(NameRequestFlags::ALLOW_REPLACEMENT)
            {
                let old = self.owners.remove(well_known).unwrap();
                self.queue
                    .entry(String::from(well_known))
                    .or_insert_with(Vec::new)
                    .push(old);
                self.owners.insert(
                    String::from(well_known),
                    NameOwner {
                        unique_name: String::from(unique),
                        flags,
                    },
                );
                return NameRequestResult::PrimaryOwner;
            }
            if flags.contains(NameRequestFlags::DO_NOT_QUEUE) {
                return NameRequestResult::Exists;
            }
            self.queue
                .entry(String::from(well_known))
                .or_insert_with(Vec::new)
                .push(NameOwner {
                    unique_name: String::from(unique),
                    flags,
                });
            NameRequestResult::InQueue
        } else {
            self.owners.insert(
                String::from(well_known),
                NameOwner {
                    unique_name: String::from(unique),
                    flags,
                },
            );
            NameRequestResult::PrimaryOwner
        }
    }

    pub fn release_name(&mut self, well_known: &str, unique: &str) -> bool {
        if let Some(owner) = self.owners.get(well_known) {
            if owner.unique_name != unique {
                return false;
            }
            self.owners.remove(well_known);
            if let Some(q) = self.queue.get_mut(well_known) {
                if let Some(next) = q.pop() {
                    self.owners.insert(String::from(well_known), next);
                }
            }
            true
        } else {
            false
        }
    }

    pub fn get_owner(&self, name: &str) -> Option<&str> {
        self.owners.get(name).map(|o| o.unique_name.as_str())
    }

    pub fn has_owner(&self, name: &str) -> bool {
        self.owners.contains_key(name)
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.owners.keys().map(|s| s.as_str()).collect()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 9. Signal Match Rules
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MatchRule {
    pub msg_type: Option<MessageType>,
    pub sender: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub path: Option<String>,
    pub path_namespace: Option<String>,
    pub destination: Option<String>,
    pub arg_matches: BTreeMap<u8, String>,
    pub arg0path: Option<String>,
    pub eavesdrop: bool,
}

impl MatchRule {
    pub fn new() -> Self {
        Self {
            msg_type: None,
            sender: None,
            interface: None,
            member: None,
            path: None,
            path_namespace: None,
            destination: None,
            arg_matches: BTreeMap::new(),
            arg0path: None,
            eavesdrop: false,
        }
    }

    pub fn matches_message(&self, msg: &DbusMessage, sender_name: Option<&str>) -> bool {
        if let Some(ref mt) = self.msg_type {
            if msg.header.msg_type != *mt {
                return false;
            }
        }
        if let Some(ref s) = self.sender {
            if sender_name != Some(s.as_str()) {
                return false;
            }
        }
        if let Some(ref iface) = self.interface {
            if let Some(HeaderFieldValue::InterfaceName(ref i)) =
                msg.header.get_field(HeaderFieldCode::Interface)
            {
                if i != iface {
                    return false;
                }
            } else {
                return false;
            }
        }
        if let Some(ref m) = self.member {
            if let Some(HeaderFieldValue::MemberName(ref mem)) =
                msg.header.get_field(HeaderFieldCode::Member)
            {
                if mem != m {
                    return false;
                }
            } else {
                return false;
            }
        }
        if let Some(ref p) = self.path {
            if let Some(HeaderFieldValue::ObjectPath(ref op)) =
                msg.header.get_field(HeaderFieldCode::Path)
            {
                if op != p {
                    return false;
                }
            } else {
                return false;
            }
        }
        if let Some(ref ns) = self.path_namespace {
            if let Some(HeaderFieldValue::ObjectPath(ref op)) =
                msg.header.get_field(HeaderFieldCode::Path)
            {
                if !op.starts_with(ns.as_str()) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 10. Bus Policy
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub effect: PolicyEffect,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub send_dest: Option<String>,
    pub send_iface: Option<String>,
    pub recv_sender: Option<String>,
    pub own: Option<String>,
    pub eavesdrop: Option<bool>,
}

impl PolicyRule {
    pub fn allow_all() -> Self {
        Self {
            effect: PolicyEffect::Allow,
            uid: None,
            gid: None,
            send_dest: None,
            send_iface: None,
            recv_sender: None,
            own: None,
            eavesdrop: None,
        }
    }

    pub fn deny_own(name: &str) -> Self {
        Self {
            effect: PolicyEffect::Deny,
            uid: None,
            gid: None,
            send_dest: None,
            send_iface: None,
            recv_sender: None,
            own: Some(String::from(name)),
            eavesdrop: None,
        }
    }
}

pub struct BusPolicy {
    pub default_rules: Vec<PolicyRule>,
    pub user_rules: BTreeMap<u32, Vec<PolicyRule>>,
    pub group_rules: BTreeMap<u32, Vec<PolicyRule>>,
}

impl BusPolicy {
    pub fn new() -> Self {
        Self {
            default_rules: Vec::new(),
            user_rules: BTreeMap::new(),
            group_rules: BTreeMap::new(),
        }
    }

    pub fn add_default_rule(&mut self, rule: PolicyRule) {
        self.default_rules.push(rule);
    }

    pub fn add_user_rule(&mut self, uid: u32, rule: PolicyRule) {
        self.user_rules
            .entry(uid)
            .or_insert_with(Vec::new)
            .push(rule);
    }

    pub fn check_send(&self, uid: u32, dest: &str, iface: &str) -> PolicyEffect {
        if let Some(rules) = self.user_rules.get(&uid) {
            for r in rules.iter().rev() {
                if let Some(ref d) = r.send_dest {
                    if d == dest {
                        if let Some(ref i) = r.send_iface {
                            if i == iface {
                                return r.effect;
                            }
                        } else {
                            return r.effect;
                        }
                    }
                }
            }
        }
        for r in self.default_rules.iter().rev() {
            if let Some(ref d) = r.send_dest {
                if d == dest {
                    return r.effect;
                }
            }
        }
        PolicyEffect::Allow
    }

    pub fn check_own(&self, uid: u32, name: &str) -> PolicyEffect {
        if let Some(rules) = self.user_rules.get(&uid) {
            for r in rules.iter().rev() {
                if let Some(ref o) = r.own {
                    if o == name || o == "*" {
                        return r.effect;
                    }
                }
            }
        }
        for r in self.default_rules.iter().rev() {
            if let Some(ref o) = r.own {
                if o == name || o == "*" {
                    return r.effect;
                }
            }
        }
        PolicyEffect::Allow
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 11. Object Path Hierarchy & Introspection
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InterfaceMethod {
    pub name: String,
    pub in_sig: String,
    pub out_sig: String,
}

#[derive(Debug, Clone)]
pub struct InterfaceSignal {
    pub name: String,
    pub sig: String,
}

#[derive(Debug, Clone)]
pub struct InterfaceProperty {
    pub name: String,
    pub sig: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Clone)]
pub struct DbusInterface {
    pub name: String,
    pub methods: Vec<InterfaceMethod>,
    pub signals: Vec<InterfaceSignal>,
    pub properties: Vec<InterfaceProperty>,
}

#[derive(Debug, Clone)]
pub struct ObjectNode {
    pub path: String,
    pub interfaces: Vec<DbusInterface>,
    pub children: Vec<String>,
}

impl ObjectNode {
    pub fn introspect_xml(&self) -> String {
        use alloc::format;
        let mut xml = String::from("<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\"\n \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">\n<node>\n");
        for iface in &self.interfaces {
            xml.push_str(&format!("  <interface name=\"{}\">\n", iface.name));
            for m in &iface.methods {
                xml.push_str(&format!("    <method name=\"{}\">\n", m.name));
                if !m.in_sig.is_empty() {
                    xml.push_str(&format!(
                        "      <arg direction=\"in\" type=\"{}\"/>\n",
                        m.in_sig
                    ));
                }
                if !m.out_sig.is_empty() {
                    xml.push_str(&format!(
                        "      <arg direction=\"out\" type=\"{}\"/>\n",
                        m.out_sig
                    ));
                }
                xml.push_str("    </method>\n");
            }
            for s in &iface.signals {
                xml.push_str(&format!("    <signal name=\"{}\">\n", s.name));
                if !s.sig.is_empty() {
                    xml.push_str(&format!("      <arg type=\"{}\"/>\n", s.sig));
                }
                xml.push_str("    </signal>\n");
            }
            for p in &iface.properties {
                let access = match (p.readable, p.writable) {
                    (true, true) => "readwrite",
                    (true, false) => "read",
                    (false, true) => "write",
                    (false, false) => "read",
                };
                xml.push_str(&format!(
                    "    <property name=\"{}\" type=\"{}\" access=\"{}\"/>\n",
                    p.name, p.sig, access
                ));
            }
            xml.push_str("  </interface>\n");
        }
        for child in &self.children {
            xml.push_str(&format!("  <node name=\"{}\"/>\n", child));
        }
        xml.push_str("</node>\n");
        xml
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 12. Service Activation
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ServiceFile {
    pub name: String,
    pub exec: String,
    pub user: Option<String>,
    pub systemd_unit: Option<String>,
}

pub struct ActivationManager {
    pub services: BTreeMap<String, ServiceFile>,
    pub pending_starts: Vec<String>,
}

impl ActivationManager {
    pub fn new() -> Self {
        Self {
            services: BTreeMap::new(),
            pending_starts: Vec::new(),
        }
    }

    pub fn register_service(&mut self, service: ServiceFile) {
        self.services.insert(service.name.clone(), service);
    }

    pub fn list_activatable(&self) -> Vec<&str> {
        self.services.keys().map(|s| s.as_str()).collect()
    }

    pub fn auto_start(&mut self, name: &str) -> Option<&ServiceFile> {
        if self.services.contains_key(name) {
            self.pending_starts.push(String::from(name));
            self.services.get(name)
        } else {
            None
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 13. Connection & Credentials
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMechanism {
    External,
    Anonymous,
}

#[derive(Debug, Clone)]
pub struct PeerCredentials {
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub security_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DbusConnection {
    pub unique_name: String,
    pub authenticated: bool,
    pub auth_method: Option<AuthMechanism>,
    pub credentials: PeerCredentials,
    pub owned_names: Vec<String>,
    pub match_rules: Vec<MatchRule>,
    pub max_msg_size: usize,
    pub msg_count: u64,
}

impl DbusConnection {
    pub fn new(unique_name: &str, cred: PeerCredentials) -> Self {
        Self {
            unique_name: String::from(unique_name),
            authenticated: false,
            auth_method: None,
            credentials: cred,
            owned_names: Vec::new(),
            match_rules: Vec::new(),
            max_msg_size: 128 * 1024 * 1024,
            msg_count: 0,
        }
    }

    pub fn authenticate(&mut self, method: AuthMechanism) {
        self.authenticated = true;
        self.auth_method = Some(method);
    }

    pub fn add_match(&mut self, rule: MatchRule) {
        self.match_rules.push(rule);
    }

    pub fn remove_match(&mut self, index: usize) -> bool {
        if index < self.match_rules.len() {
            self.match_rules.remove(index);
            true
        } else {
            false
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 14. Global D-Bus Bus
// ───────────────────────────────────────────────────────────────────────────────

pub struct DbusBus {
    pub bus_type: BusType,
    pub bus_id: [u8; 16],
    pub names: NameRegistry,
    pub policy: BusPolicy,
    pub connections: BTreeMap<String, DbusConnection>,
    pub activation: ActivationManager,
    pub objects: BTreeMap<String, ObjectNode>,
    pub next_serial: u32,
    pub next_conn: u64,
    pub max_connections: usize,
    pub total_messages: u64,
}

impl DbusBus {
    pub fn new(bus_type: BusType) -> Self {
        Self {
            bus_type,
            bus_id: [
                0x52, 0x61, 0x65, 0x65, 0x6e, 0x4f, 0x53, 0x2d, 0x44, 0x42, 0x75, 0x73, 0x2d, 0x49,
                0x44, 0x31,
            ],
            names: NameRegistry::new(),
            policy: BusPolicy::new(),
            connections: BTreeMap::new(),
            activation: ActivationManager::new(),
            objects: BTreeMap::new(),
            next_serial: 1,
            next_conn: 1,
            max_connections: 256,
            total_messages: 0,
        }
    }

    fn alloc_serial(&mut self) -> u32 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    fn alloc_unique_name(&mut self) -> String {
        use alloc::format;
        let n = self.next_conn;
        self.next_conn += 1;
        format!(":1.{}", n)
    }

    pub fn hello(&mut self, cred: PeerCredentials) -> Result<String, &'static str> {
        if self.connections.len() >= self.max_connections {
            return Err("max connections reached");
        }
        let name = self.alloc_unique_name();
        let mut conn = DbusConnection::new(&name, cred);
        conn.authenticate(AuthMechanism::External);
        self.connections.insert(name.clone(), conn);
        Ok(name)
    }

    pub fn request_name(
        &mut self,
        unique: &str,
        well_known: &str,
        flags: NameRequestFlags,
    ) -> Result<NameRequestResult, &'static str> {
        if !self.connections.contains_key(unique) {
            return Err("not connected");
        }
        let uid = self.connections[unique].credentials.uid;
        if self.policy.check_own(uid, well_known) == PolicyEffect::Deny {
            return Err("policy denied");
        }
        let result = self.names.request_name(well_known, unique, flags);
        if result == NameRequestResult::PrimaryOwner {
            if let Some(conn) = self.connections.get_mut(unique) {
                conn.owned_names.push(String::from(well_known));
            }
        }
        Ok(result)
    }

    pub fn release_name(&mut self, unique: &str, well_known: &str) -> bool {
        if self.names.release_name(well_known, unique) {
            if let Some(conn) = self.connections.get_mut(unique) {
                conn.owned_names.retain(|n| n != well_known);
            }
            true
        } else {
            false
        }
    }

    pub fn send_message(&mut self, from: &str, msg: DbusMessage) -> Result<(), &'static str> {
        if !self.connections.contains_key(from) {
            return Err("sender not connected");
        }
        if let Some(conn) = self.connections.get_mut(from) {
            conn.msg_count += 1;
        }
        self.total_messages += 1;

        if msg.header.msg_type == MessageType::Signal {
            self.dispatch_signal(from, &msg);
        } else if let Some(HeaderFieldValue::BusName(ref dest)) =
            msg.header.get_field(HeaderFieldCode::Destination)
        {
            let dest_unique = if dest.starts_with(':') {
                dest.clone()
            } else {
                match self.names.get_owner(dest) {
                    Some(u) => String::from(u),
                    None => {
                        if let Some(_svc) = self.activation.auto_start(dest) {
                            return Ok(());
                        }
                        return Err("destination not found");
                    }
                }
            };
            if !self.connections.contains_key(&dest_unique) {
                return Err("destination connection gone");
            }
        }
        Ok(())
    }

    fn dispatch_signal(&self, sender: &str, msg: &DbusMessage) {
        for (_name, conn) in &self.connections {
            for rule in &conn.match_rules {
                if rule.matches_message(msg, Some(sender)) {
                    break;
                }
            }
        }
    }

    pub fn disconnect(&mut self, unique: &str) {
        if let Some(conn) = self.connections.remove(unique) {
            for name in &conn.owned_names {
                self.names.release_name(name, unique);
            }
        }
    }

    pub fn get_id(&self) -> String {
        let mut s = String::with_capacity(32);
        for b in &self.bus_id {
            use core::fmt::Write;
            let _ = write!(s, "{:02x}", b);
        }
        s
    }

    pub fn list_names(&self) -> Vec<&str> {
        let mut names = self.names.list_names();
        for k in self.connections.keys() {
            names.push(k.as_str());
        }
        names
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

pub static DBUS_BUS: Mutex<Option<DbusBus>> = Mutex::new(None);

pub fn init() {
    let mut bus = DBUS_BUS.lock();
    *bus = Some(DbusBus::new(BusType::System));
}
