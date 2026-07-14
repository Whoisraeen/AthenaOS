//! wevtapi.dll — Windows Event Log API, legacy event log, channel configuration,
//! subscription, publisher metadata, and event rendering for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    WinHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, INVALID_HANDLE_VALUE, NULL_HANDLE,
};

// =========================================================================
// Event levels
// =========================================================================

pub const EVT_LEVEL_CRITICAL: u8 = 1;
pub const EVT_LEVEL_ERROR: u8 = 2;
pub const EVT_LEVEL_WARNING: u8 = 3;
pub const EVT_LEVEL_INFORMATION: u8 = 4;
pub const EVT_LEVEL_VERBOSE: u8 = 5;

// =========================================================================
// Event render flags
// =========================================================================

pub const EVT_RENDER_EVENT_VALUES: u32 = 0;
pub const EVT_RENDER_EVENT_XML: u32 = 1;
pub const EVT_RENDER_BOOKMARK: u32 = 2;

// =========================================================================
// Event query flags
// =========================================================================

pub const EVT_QUERY_CHANNEL_PATH: u32 = 0x1;
pub const EVT_QUERY_FILE_PATH: u32 = 0x2;
pub const EVT_QUERY_FORWARD_DIRECTION: u32 = 0x100;
pub const EVT_QUERY_REVERSE_DIRECTION: u32 = 0x200;
pub const EVT_QUERY_TOLERATE_QUERY_ERRORS: u32 = 0x1000;

// =========================================================================
// Event seek flags
// =========================================================================

pub const EVT_SEEK_RELATIVE_TO_FIRST: u32 = 1;
pub const EVT_SEEK_RELATIVE_TO_LAST: u32 = 2;
pub const EVT_SEEK_RELATIVE_TO_CURRENT: u32 = 3;
pub const EVT_SEEK_RELATIVE_TO_BOOKMARK: u32 = 4;
pub const EVT_SEEK_ORIGIN_MASK: u32 = 0x7;
pub const EVT_SEEK_STRICT: u32 = 0x10000;

// =========================================================================
// Subscription flags
// =========================================================================

pub const EVT_SUBSCRIBE_TO_FUTURE_EVENTS: u32 = 1;
pub const EVT_SUBSCRIBE_START_AT_OLDEST_RECORD: u32 = 2;
pub const EVT_SUBSCRIBE_START_AFTER_BOOKMARK: u32 = 3;
pub const EVT_SUBSCRIBE_STRICT: u32 = 0x10000;

// =========================================================================
// Channel config property IDs
// =========================================================================

pub const EVT_CHANNEL_CONFIG_ENABLED: u32 = 0;
pub const EVT_CHANNEL_CONFIG_ISOLATION: u32 = 1;
pub const EVT_CHANNEL_CONFIG_TYPE: u32 = 2;
pub const EVT_CHANNEL_CONFIG_OWNING_PUBLISHER: u32 = 3;
pub const EVT_CHANNEL_CONFIG_CLASS_ADMIN: u32 = 4;
pub const EVT_CHANNEL_CONFIG_CLASS_OPERATIONAL: u32 = 5;
pub const EVT_CHANNEL_CONFIG_CLASS_ANALYTIC: u32 = 6;
pub const EVT_CHANNEL_CONFIG_CLASS_DEBUG: u32 = 7;
pub const EVT_CHANNEL_CONFIG_LOG_FILE_PATH: u32 = 8;
pub const EVT_CHANNEL_CONFIG_MAX_SIZE: u32 = 9;
pub const EVT_CHANNEL_CONFIG_RETENTION: u32 = 10;
pub const EVT_CHANNEL_CONFIG_AUTO_BACKUP: u32 = 11;

// =========================================================================
// Publisher metadata property IDs
// =========================================================================

pub const EVT_PUBLISHER_METADATA_PUBLISHER_GUID: u32 = 0;
pub const EVT_PUBLISHER_METADATA_RESOURCE_FILE_PATH: u32 = 1;
pub const EVT_PUBLISHER_METADATA_PARAMETER_FILE_PATH: u32 = 2;
pub const EVT_PUBLISHER_METADATA_MESSAGE_FILE_PATH: u32 = 3;
pub const EVT_PUBLISHER_METADATA_HELP_LINK: u32 = 4;
pub const EVT_PUBLISHER_METADATA_PUBLISHER_MESSAGE_ID: u32 = 5;
pub const EVT_PUBLISHER_METADATA_CHANNEL_REFERENCES: u32 = 6;
pub const EVT_PUBLISHER_METADATA_LEVELS: u32 = 7;
pub const EVT_PUBLISHER_METADATA_TASKS: u32 = 8;
pub const EVT_PUBLISHER_METADATA_OPCODES: u32 = 9;
pub const EVT_PUBLISHER_METADATA_KEYWORDS: u32 = 10;

// =========================================================================
// Event metadata property IDs
// =========================================================================

pub const EVT_EVENT_METADATA_ID: u32 = 0;
pub const EVT_EVENT_METADATA_VERSION: u32 = 1;
pub const EVT_EVENT_METADATA_CHANNEL: u32 = 2;
pub const EVT_EVENT_METADATA_LEVEL: u32 = 3;
pub const EVT_EVENT_METADATA_OPCODE: u32 = 4;
pub const EVT_EVENT_METADATA_TASK: u32 = 5;
pub const EVT_EVENT_METADATA_KEYWORD: u32 = 6;
pub const EVT_EVENT_METADATA_MESSAGE_ID: u32 = 7;
pub const EVT_EVENT_METADATA_TEMPLATE: u32 = 8;

// =========================================================================
// Legacy event log types
// =========================================================================

pub const EVENTLOG_SUCCESS: u16 = 0x0000;
pub const EVENTLOG_ERROR_TYPE: u16 = 0x0001;
pub const EVENTLOG_WARNING_TYPE: u16 = 0x0002;
pub const EVENTLOG_INFORMATION_TYPE: u16 = 0x0004;
pub const EVENTLOG_AUDIT_SUCCESS: u16 = 0x0008;
pub const EVENTLOG_AUDIT_FAILURE: u16 = 0x0010;

// Legacy read flags
pub const EVENTLOG_SEQUENTIAL_READ: u32 = 0x0001;
pub const EVENTLOG_SEEK_READ: u32 = 0x0002;
pub const EVENTLOG_FORWARDS_READ: u32 = 0x0004;
pub const EVENTLOG_BACKWARDS_READ: u32 = 0x0008;

// =========================================================================
// Format message flags
// =========================================================================

pub const EVT_FORMAT_MESSAGE_EVENT: u32 = 1;
pub const EVT_FORMAT_MESSAGE_LEVEL: u32 = 2;
pub const EVT_FORMAT_MESSAGE_TASK: u32 = 3;
pub const EVT_FORMAT_MESSAGE_OPCODE: u32 = 4;
pub const EVT_FORMAT_MESSAGE_KEYWORD: u32 = 5;
pub const EVT_FORMAT_MESSAGE_CHANNEL: u32 = 6;
pub const EVT_FORMAT_MESSAGE_PROVIDER: u32 = 7;
pub const EVT_FORMAT_MESSAGE_ID: u32 = 8;
pub const EVT_FORMAT_MESSAGE_XML: u32 = 9;

// =========================================================================
// Well-known channel names
// =========================================================================

pub const CHANNEL_APPLICATION: &str = "Application";
pub const CHANNEL_SYSTEM: &str = "System";
pub const CHANNEL_SECURITY: &str = "Security";
pub const CHANNEL_SETUP: &str = "Setup";
pub const CHANNEL_FORWARDED_EVENTS: &str = "ForwardedEvents";
pub const CHANNEL_WINDOWS_POWERSHELL: &str = "Microsoft-Windows-PowerShell/Operational";
pub const CHANNEL_WINDOWS_SYSMON: &str = "Microsoft-Windows-Sysmon/Operational";
pub const CHANNEL_WINDOWS_TASKSCHEDULER: &str = "Microsoft-Windows-TaskScheduler/Operational";
pub const CHANNEL_WINDOWS_WINRM: &str = "Microsoft-Windows-WinRM/Operational";
pub const CHANNEL_WINDOWS_DEFENDER: &str = "Microsoft-Windows-Windows Defender/Operational";

// =========================================================================
// EvtVariant — typed event data
// =========================================================================

#[derive(Debug, Clone)]
pub enum EvtVariant {
    Null,
    StringVal(String),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Boolean(bool),
    Single(u32),
    Double(u64),
    Binary(Vec<u8>),
    Guid([u8; 16]),
    SystemTime(u64),
    FileTime(u64),
    Sid(Vec<u8>),
    HexInt32(u32),
    HexInt64(u64),
    StringArray(Vec<String>),
    UInt32Array(Vec<u32>),
    UInt64Array(Vec<u64>),
}

impl EvtVariant {
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::StringVal(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            Self::UInt32(v) => Some(*v),
            Self::HexInt32(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::UInt64(v) => Some(*v),
            Self::HexInt64(v) => Some(*v),
            Self::FileTime(v) => Some(*v),
            Self::SystemTime(v) => Some(*v),
            _ => None,
        }
    }
}

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct EventDescriptor {
    pub id: u16,
    pub version: u8,
    pub channel: u8,
    pub level: u8,
    pub opcode: u8,
    pub task: u16,
    pub keyword: u64,
}

#[derive(Debug, Clone)]
pub struct EventRecord {
    pub handle: WinHandle,
    pub event_id: u16,
    pub level: u8,
    pub channel: String,
    pub provider: String,
    pub time_created: u64,
    pub computer: String,
    pub message: String,
    pub data: Vec<EvtVariant>,
    pub xml: String,
}

#[derive(Debug, Clone)]
struct ChannelConfig {
    name: String,
    enabled: bool,
    isolation: u32,
    channel_type: u32,
    log_file_path: String,
    max_size: u64,
    retention: bool,
}

struct EventQuery {
    handle: WinHandle,
    channel: String,
    query: String,
    flags: u32,
    position: usize,
}

struct EventSubscription {
    handle: WinHandle,
    channel: String,
    query: String,
    flags: u32,
    callback: u64,
    bookmark: Option<WinHandle>,
}

struct LegacyEventLog {
    handle: WinHandle,
    source: String,
    records: Vec<EventRecord>,
    position: usize,
}

struct PublisherMetadata {
    handle: WinHandle,
    publisher_id: String,
    resource_file: String,
    message_file: String,
}

struct RenderContext {
    handle: WinHandle,
    paths: Vec<String>,
}

// =========================================================================
// Global state
// =========================================================================

pub struct EventLog {
    next_handle: u64,
    channels: BTreeMap<String, ChannelConfig>,
    events: BTreeMap<String, Vec<EventRecord>>,
    queries: BTreeMap<u64, EventQuery>,
    subscriptions: BTreeMap<u64, EventSubscription>,
    legacy_logs: BTreeMap<u64, LegacyEventLog>,
    legacy_sources: BTreeMap<u64, String>,
    publishers: BTreeMap<u64, PublisherMetadata>,
    render_contexts: BTreeMap<u64, RenderContext>,
    bookmarks: BTreeMap<u64, (String, u64)>,
    next_record_id: u64,
}

impl EventLog {
    const fn new() -> Self {
        Self {
            next_handle: 0xE000_0000,
            channels: BTreeMap::new(),
            events: BTreeMap::new(),
            queries: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            legacy_logs: BTreeMap::new(),
            legacy_sources: BTreeMap::new(),
            publishers: BTreeMap::new(),
            render_contexts: BTreeMap::new(),
            bookmarks: BTreeMap::new(),
            next_record_id: 1,
        }
    }

    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }

    fn populate_defaults(&mut self) {
        let channels = [
            (CHANNEL_APPLICATION, "Application", 0x10000u64),
            (CHANNEL_SYSTEM, "System", 0x10000),
            (CHANNEL_SECURITY, "Security", 0x10000),
            (CHANNEL_SETUP, "Setup", 0x10000),
            (CHANNEL_FORWARDED_EVENTS, "ForwardedEvents", 0x10000),
        ];

        for (name, _display, max) in channels {
            self.channels.insert(
                String::from(name),
                ChannelConfig {
                    name: String::from(name),
                    enabled: true,
                    isolation: 0,
                    channel_type: 1,
                    log_file_path: {
                        let mut p = String::from("C:\\Windows\\System32\\winevt\\Logs\\");
                        p.push_str(name);
                        p.push_str(".evtx");
                        p
                    },
                    max_size: max,
                    retention: false,
                },
            );
            self.events.insert(String::from(name), Vec::new());
        }

        let seed_events = [
            (
                CHANNEL_SYSTEM,
                6013u16,
                EVT_LEVEL_INFORMATION,
                "EventLog",
                "The system uptime is 86400 seconds.",
            ),
            (
                CHANNEL_SYSTEM,
                7036,
                EVT_LEVEL_INFORMATION,
                "Service Control Manager",
                "The Windows Update service entered the running state.",
            ),
            (
                CHANNEL_APPLICATION,
                1000,
                EVT_LEVEL_ERROR,
                "Application Error",
                "Faulting application name: explorer.exe",
            ),
            (
                CHANNEL_APPLICATION,
                1026,
                EVT_LEVEL_ERROR,
                ".NET Runtime",
                "Application: app.exe Framework Version: v4.0.30319",
            ),
            (
                CHANNEL_SECURITY,
                4624,
                EVT_LEVEL_INFORMATION,
                "Microsoft-Windows-Security-Auditing",
                "An account was successfully logged on.",
            ),
            (
                CHANNEL_SECURITY,
                4625,
                EVT_LEVEL_WARNING,
                "Microsoft-Windows-Security-Auditing",
                "An account failed to log on.",
            ),
        ];

        for (chan, id, level, provider, msg) in seed_events {
            let rec_handle = self.alloc_handle();
            let rid = self.next_record_id;
            self.next_record_id += 1;
            let record = EventRecord {
                handle: rec_handle,
                event_id: id,
                level,
                channel: String::from(chan),
                provider: String::from(provider),
                time_created: 133_600_000_000_000_000 + rid * 10_000_000,
                computer: String::from("RAEENOS-PC"),
                message: String::from(msg),
                data: Vec::new(),
                xml: build_event_xml(chan, id, level, provider, msg, rid),
            };
            if let Some(list) = self.events.get_mut(chan) {
                list.push(record);
            }
        }
    }
}

fn build_event_xml(
    channel: &str,
    id: u16,
    level: u8,
    provider: &str,
    msg: &str,
    rid: u64,
) -> String {
    let mut x =
        String::from("<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'>");
    x.push_str("<System><Provider Name='");
    x.push_str(provider);
    x.push_str("'/><EventID>");
    push_u16(&mut x, id);
    x.push_str("</EventID><Level>");
    push_u8(&mut x, level);
    x.push_str("</Level><Channel>");
    x.push_str(channel);
    x.push_str("</Channel><EventRecordID>");
    push_u64(&mut x, rid);
    x.push_str("</EventRecordID></System><EventData><Data>");
    x.push_str(msg);
    x.push_str("</Data></EventData></Event>");
    x
}

fn push_u8(s: &mut String, v: u8) {
    let mut buf = [0u8; 3];
    let n = fmt_u64(v as u64, &mut buf);
    if let Ok(t) = core::str::from_utf8(&buf[..n]) {
        s.push_str(t);
    }
}

fn push_u16(s: &mut String, v: u16) {
    let mut buf = [0u8; 5];
    let n = fmt_u64(v as u64, &mut buf);
    if let Ok(t) = core::str::from_utf8(&buf[..n]) {
        s.push_str(t);
    }
}

fn push_u64(s: &mut String, v: u64) {
    let mut buf = [0u8; 20];
    let n = fmt_u64(v, &mut buf);
    if let Ok(t) = core::str::from_utf8(&buf[..n]) {
        s.push_str(t);
    }
}

fn fmt_u64(mut v: u64, buf: &mut [u8]) -> usize {
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

static mut EVENT_LOG: Option<EventLog> = None;

pub fn init() {
    unsafe {
        let mut el = EventLog::new();
        el.populate_defaults();
        EVENT_LOG = Some(el);
    }
}

fn elog() -> &'static mut EventLog {
    unsafe {
        EVENT_LOG
            .as_mut()
            .expect("wevtapi not initialized — call init()")
    }
}

// =========================================================================
// Modern Event Log API
// =========================================================================

pub fn evt_open_log(_session: WinHandle, path: &str, flags: u32) -> WinHandle {
    let el = elog();
    if flags & EVT_QUERY_FILE_PATH != 0 || el.channels.contains_key(path) {
        el.alloc_handle()
    } else {
        INVALID_HANDLE_VALUE
    }
}

pub fn evt_close(handle: WinHandle) -> bool {
    let el = elog();
    let h = handle.0;
    el.queries.remove(&h).is_some()
        || el.subscriptions.remove(&h).is_some()
        || el.render_contexts.remove(&h).is_some()
        || el.bookmarks.remove(&h).is_some()
        || el.publishers.remove(&h).is_some()
        || el.legacy_logs.remove(&h).is_some()
        || el.legacy_sources.remove(&h).is_some()
        || h >= 0xE000_0000
}

pub fn evt_query(_session: WinHandle, path: &str, query: &str, flags: u32) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    el.queries.insert(
        handle.0,
        EventQuery {
            handle,
            channel: String::from(path),
            query: String::from(query),
            flags,
            position: 0,
        },
    );
    handle
}

pub fn evt_next(
    query_handle: WinHandle,
    events_buf: &mut Vec<WinHandle>,
    count: u32,
    _timeout: u32,
    returned: &mut u32,
) -> bool {
    let el = elog();
    let q = match el.queries.get_mut(&query_handle.0) {
        Some(q) => q,
        None => return false,
    };
    let channel = q.channel.clone();
    let pos = q.position;
    let records = match el.events.get(&channel) {
        Some(r) => r,
        None => return false,
    };
    if pos >= records.len() {
        *returned = 0;
        return false;
    }
    let end = (pos + count as usize).min(records.len());
    let mut n = 0u32;
    for rec in &records[pos..end] {
        events_buf.push(rec.handle);
        n += 1;
    }
    *returned = n;
    let q = el.queries.get_mut(&query_handle.0).unwrap();
    q.position = end;
    true
}

pub fn evt_seek(
    query_handle: WinHandle,
    position: i64,
    _bookmark: WinHandle,
    _timeout: u32,
    flags: u32,
) -> bool {
    let el = elog();
    let q = match el.queries.get_mut(&query_handle.0) {
        Some(q) => q,
        None => return false,
    };
    let chan = q.channel.clone();
    let total = el.events.get(&chan).map(|v| v.len()).unwrap_or(0);
    let q = el.queries.get_mut(&query_handle.0).unwrap();
    match flags & EVT_SEEK_ORIGIN_MASK {
        1 => q.position = position.max(0) as usize,
        2 => {
            q.position = if position < 0 {
                total.saturating_sub((-position) as usize)
            } else {
                total
            };
        }
        3 => {
            let cur = q.position as i64;
            q.position = (cur + position).max(0) as usize;
        }
        _ => return false,
    }
    true
}

pub fn evt_create_render_context(paths: &[&str], _flags: u32) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    let paths_vec: Vec<String> = paths.iter().map(|p| String::from(*p)).collect();
    el.render_contexts.insert(
        handle.0,
        RenderContext {
            handle,
            paths: paths_vec,
        },
    );
    handle
}

pub fn evt_render(
    _context: WinHandle,
    event: WinHandle,
    flags: u32,
    buf: &mut [u8],
    buf_size: u32,
    used: &mut u32,
    property_count: &mut u32,
) -> bool {
    let el = elog();
    let mut found: Option<&EventRecord> = None;
    for records in el.events.values() {
        for rec in records {
            if rec.handle.0 == event.0 {
                found = Some(rec);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let record = match found {
        Some(r) => r,
        None => return false,
    };

    match flags {
        EVT_RENDER_EVENT_XML => {
            let bytes = record.xml.as_bytes();
            *used = bytes.len() as u32;
            if buf_size < *used || buf.len() < bytes.len() {
                return false;
            }
            buf[..bytes.len()].copy_from_slice(bytes);
            *property_count = 0;
            true
        }
        EVT_RENDER_EVENT_VALUES => {
            *used = 0;
            *property_count = record.data.len() as u32;
            true
        }
        _ => false,
    }
}

pub fn evt_format_message(
    _publisher: WinHandle,
    event: WinHandle,
    _message_id: u32,
    _values_count: u32,
    flags: u32,
    buf: &mut [u16],
    buf_size: u32,
    used: &mut u32,
) -> bool {
    let el = elog();
    let mut found: Option<&EventRecord> = None;
    for records in el.events.values() {
        for rec in records {
            if rec.handle.0 == event.0 {
                found = Some(rec);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let record = match found {
        Some(r) => r,
        None => return false,
    };
    let text = match flags {
        EVT_FORMAT_MESSAGE_EVENT => &record.message,
        EVT_FORMAT_MESSAGE_CHANNEL => &record.channel,
        EVT_FORMAT_MESSAGE_PROVIDER => &record.provider,
        EVT_FORMAT_MESSAGE_LEVEL => {
            let s = match record.level {
                EVT_LEVEL_CRITICAL => "Critical",
                EVT_LEVEL_ERROR => "Error",
                EVT_LEVEL_WARNING => "Warning",
                EVT_LEVEL_INFORMATION => "Information",
                EVT_LEVEL_VERBOSE => "Verbose",
                _ => "Unknown",
            };
            let wide = crate::string_to_wide(s);
            *used = wide.len() as u32;
            if (buf_size as usize) < wide.len() || buf.len() < wide.len() {
                return false;
            }
            let n = wide.len().min(buf.len());
            buf[..n].copy_from_slice(&wide[..n]);
            return true;
        }
        _ => return false,
    };
    let wide = crate::string_to_wide(text);
    *used = wide.len() as u32;
    if (buf_size as usize) < wide.len() || buf.len() < wide.len() {
        return false;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    true
}

// =========================================================================
// Event creation
// =========================================================================

pub fn evt_write_event(
    channel: &str,
    descriptor: &EventDescriptor,
    message: &str,
    data: Vec<EvtVariant>,
) -> bool {
    let el = elog();
    let handle = el.alloc_handle();
    let rid = el.next_record_id;
    el.next_record_id += 1;
    let record = EventRecord {
        handle,
        event_id: descriptor.id,
        level: descriptor.level,
        channel: String::from(channel),
        provider: String::from("RaeenOS-App"),
        time_created: 133_600_000_000_000_000 + rid * 10_000_000,
        computer: String::from("RAEENOS-PC"),
        message: String::from(message),
        data,
        xml: build_event_xml(
            channel,
            descriptor.id,
            descriptor.level,
            "RaeenOS-App",
            message,
            rid,
        ),
    };
    if let Some(list) = el.events.get_mut(channel) {
        list.push(record);
        true
    } else {
        false
    }
}

// =========================================================================
// Bookmarks
// =========================================================================

pub fn evt_create_bookmark(xml: Option<&str>) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    let channel = match xml {
        Some(x) => String::from(x),
        None => String::from(CHANNEL_APPLICATION),
    };
    el.bookmarks.insert(handle.0, (channel, 0));
    handle
}

pub fn evt_update_bookmark(bookmark: WinHandle, event: WinHandle) -> bool {
    let el = elog();
    if let Some(entry) = el.bookmarks.get_mut(&bookmark.0) {
        entry.1 = event.0;
        true
    } else {
        false
    }
}

// =========================================================================
// Subscription
// =========================================================================

pub fn evt_subscribe(
    _session: WinHandle,
    _signal_event: WinHandle,
    channel: &str,
    query: &str,
    bookmark: WinHandle,
    callback: u64,
    flags: u32,
) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    let bm = if bookmark.0 != 0 {
        Some(bookmark)
    } else {
        None
    };
    el.subscriptions.insert(
        handle.0,
        EventSubscription {
            handle,
            channel: String::from(channel),
            query: String::from(query),
            flags,
            callback,
            bookmark: bm,
        },
    );
    handle
}

// =========================================================================
// Legacy Event Log API
// =========================================================================

pub fn open_event_log_w(_server: Option<&str>, source: &str) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    let records = el.events.get(source).cloned().unwrap_or_default();
    el.legacy_logs.insert(
        handle.0,
        LegacyEventLog {
            handle,
            source: String::from(source),
            records,
            position: 0,
        },
    );
    handle
}

pub fn read_event_log_w(
    handle: WinHandle,
    flags: u32,
    _record_offset: u32,
    buf: &mut [u8],
    buf_size: u32,
    bytes_read: &mut u32,
    min_bytes_needed: &mut u32,
) -> bool {
    let el = elog();
    let log = match el.legacy_logs.get_mut(&handle.0) {
        Some(l) => l,
        None => return false,
    };
    if log.position >= log.records.len() {
        *bytes_read = 0;
        return false;
    }
    let rec = &log.records[log.position];
    let msg_bytes = rec.message.as_bytes();
    let needed = msg_bytes.len() as u32 + 56;
    *min_bytes_needed = needed;
    if buf_size < needed || buf.len() < needed as usize {
        return false;
    }
    let n = msg_bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&msg_bytes[..n]);
    *bytes_read = n as u32;
    if flags & EVENTLOG_FORWARDS_READ != 0 {
        log.position += 1;
    }
    true
}

pub fn report_event_w(
    source_handle: WinHandle,
    event_type: u16,
    _category: u16,
    event_id: u32,
    _user_sid: u64,
    message: &str,
) -> bool {
    let el = elog();
    let source = match el.legacy_sources.get(&source_handle.0) {
        Some(s) => s.clone(),
        None => String::from("Application"),
    };
    let level = match event_type {
        EVENTLOG_ERROR_TYPE => EVT_LEVEL_ERROR,
        EVENTLOG_WARNING_TYPE => EVT_LEVEL_WARNING,
        EVENTLOG_INFORMATION_TYPE | EVENTLOG_SUCCESS => EVT_LEVEL_INFORMATION,
        EVENTLOG_AUDIT_SUCCESS | EVENTLOG_AUDIT_FAILURE => EVT_LEVEL_INFORMATION,
        _ => EVT_LEVEL_INFORMATION,
    };
    let desc = EventDescriptor {
        id: event_id as u16,
        version: 0,
        channel: 0,
        level,
        opcode: 0,
        task: 0,
        keyword: 0,
    };
    evt_write_event(&source, &desc, message, Vec::new())
}

pub fn clear_event_log_w(handle: WinHandle, _backup_file: Option<&str>) -> bool {
    let el = elog();
    let source = match el.legacy_logs.get(&handle.0) {
        Some(l) => l.source.clone(),
        None => return false,
    };
    if let Some(list) = el.events.get_mut(&source) {
        list.clear();
    }
    if let Some(log) = el.legacy_logs.get_mut(&handle.0) {
        log.records.clear();
        log.position = 0;
    }
    true
}

pub fn get_number_of_event_log_records(handle: WinHandle, count: &mut u32) -> bool {
    let el = elog();
    match el.legacy_logs.get(&handle.0) {
        Some(log) => {
            *count = log.records.len() as u32;
            true
        }
        None => false,
    }
}

pub fn get_oldest_event_log_record(handle: WinHandle, oldest: &mut u32) -> bool {
    let el = elog();
    match el.legacy_logs.get(&handle.0) {
        Some(_) => {
            *oldest = 1;
            true
        }
        None => false,
    }
}

pub fn register_event_source_w(_server: Option<&str>, source: &str) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    el.legacy_sources.insert(handle.0, String::from(source));
    handle
}

pub fn deregister_event_source(handle: WinHandle) -> bool {
    elog().legacy_sources.remove(&handle.0).is_some()
}

// =========================================================================
// Publisher metadata
// =========================================================================

pub fn evt_open_publisher_metadata(
    _session: WinHandle,
    publisher_id: &str,
    _log_file: Option<&str>,
    _locale: u32,
) -> WinHandle {
    let el = elog();
    let handle = el.alloc_handle();
    el.publishers.insert(
        handle.0,
        PublisherMetadata {
            handle,
            publisher_id: String::from(publisher_id),
            resource_file: String::from("C:\\Windows\\System32\\wevtapi.dll"),
            message_file: String::from("C:\\Windows\\System32\\wevtapi.dll"),
        },
    );
    handle
}

pub fn evt_get_publisher_metadata_property(
    publisher: WinHandle,
    property_id: u32,
    result: &mut EvtVariant,
) -> bool {
    let el = elog();
    let meta = match el.publishers.get(&publisher.0) {
        Some(m) => m,
        None => return false,
    };
    match property_id {
        EVT_PUBLISHER_METADATA_PUBLISHER_GUID => {
            *result = EvtVariant::Guid([0u8; 16]);
            true
        }
        EVT_PUBLISHER_METADATA_RESOURCE_FILE_PATH => {
            *result = EvtVariant::StringVal(meta.resource_file.clone());
            true
        }
        EVT_PUBLISHER_METADATA_MESSAGE_FILE_PATH => {
            *result = EvtVariant::StringVal(meta.message_file.clone());
            true
        }
        _ => false,
    }
}

pub fn evt_open_event_metadata_enum(_publisher: WinHandle, _flags: u32) -> WinHandle {
    elog().alloc_handle()
}

pub fn evt_next_event_metadata(_enum_handle: WinHandle, _flags: u32) -> WinHandle {
    INVALID_HANDLE_VALUE
}

pub fn evt_get_event_metadata_property(
    _metadata: WinHandle,
    _property_id: u32,
    result: &mut EvtVariant,
) -> bool {
    *result = EvtVariant::Null;
    false
}

// =========================================================================
// Channel configuration
// =========================================================================

pub fn evt_open_channel_config(_session: WinHandle, channel: &str, _flags: u32) -> WinHandle {
    let el = elog();
    if el.channels.contains_key(channel) {
        el.alloc_handle()
    } else {
        INVALID_HANDLE_VALUE
    }
}

pub fn evt_get_channel_config_property(
    channel_name: &str,
    property_id: u32,
    result: &mut EvtVariant,
) -> bool {
    let el = elog();
    let config = match el.channels.get(channel_name) {
        Some(c) => c,
        None => return false,
    };
    match property_id {
        EVT_CHANNEL_CONFIG_ENABLED => {
            *result = EvtVariant::Boolean(config.enabled);
            true
        }
        EVT_CHANNEL_CONFIG_ISOLATION => {
            *result = EvtVariant::UInt32(config.isolation);
            true
        }
        EVT_CHANNEL_CONFIG_TYPE => {
            *result = EvtVariant::UInt32(config.channel_type);
            true
        }
        EVT_CHANNEL_CONFIG_LOG_FILE_PATH => {
            *result = EvtVariant::StringVal(config.log_file_path.clone());
            true
        }
        EVT_CHANNEL_CONFIG_MAX_SIZE => {
            *result = EvtVariant::UInt64(config.max_size);
            true
        }
        EVT_CHANNEL_CONFIG_RETENTION => {
            *result = EvtVariant::Boolean(config.retention);
            true
        }
        _ => false,
    }
}

pub fn evt_set_channel_config_property(
    channel_name: &str,
    property_id: u32,
    value: &EvtVariant,
) -> bool {
    let el = elog();
    let config = match el.channels.get_mut(channel_name) {
        Some(c) => c,
        None => return false,
    };
    match property_id {
        EVT_CHANNEL_CONFIG_ENABLED => {
            if let EvtVariant::Boolean(v) = value {
                config.enabled = *v;
                true
            } else {
                false
            }
        }
        EVT_CHANNEL_CONFIG_MAX_SIZE => {
            if let Some(v) = value.as_u64() {
                config.max_size = v;
                true
            } else {
                false
            }
        }
        EVT_CHANNEL_CONFIG_RETENTION => {
            if let EvtVariant::Boolean(v) = value {
                config.retention = *v;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}
