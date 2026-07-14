//! Linux-compatible audit framework for RaeenOS.
//!
//! Provides a full audit subsystem modelled after the Linux kernel's audit
//! infrastructure: event types, field-based filtering, audit rules with
//! operators, multiple filter lists, structured audit records, a ring-buffer
//! log with rotation, auditctl-style operations, file watches, netlink-based
//! daemon communication, session tracking, and IMA-style integrity
//! measurement.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ───────────────────────────────────────────────────────────────────────────────
// 1. Audit Event Types
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum AuditEventType {
    Syscall = 1300,
    Path = 1302,
    Cwd = 1307,
    Execve = 1309,
    Ipc = 1310,
    Socketcall = 1311,
    Sockaddr = 1306,
    FdPair = 1317,
    BprmFcaps = 1321,
    Capset = 1322,
    Mmap = 1323,
    NetfilterPkt = 1324,
    Proctitle = 1327,
    UserAuth = 1100,
    UserAcct = 1101,
    UserMgmt = 1102,
    UserLogin = 1112,
    UserLogout = 1113,
    CredAcq = 1103,
    CredDisp = 1104,
    CredRefr = 1105,
    UserCmd = 1106,
    UserTty = 1107,
    UserAvc = 1109,
    UserSelinuxErr = 1108,
    Login = 1006,
    ConfigChange = 1305,
    Anomaly = 1700,
    Integrity = 1800,
    Kernel = 2000,
    Seccomp = 1326,
}

impl AuditEventType {
    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1300 => Some(Self::Syscall),
            1302 => Some(Self::Path),
            1307 => Some(Self::Cwd),
            1309 => Some(Self::Execve),
            1310 => Some(Self::Ipc),
            1311 => Some(Self::Socketcall),
            1306 => Some(Self::Sockaddr),
            1317 => Some(Self::FdPair),
            1321 => Some(Self::BprmFcaps),
            1322 => Some(Self::Capset),
            1323 => Some(Self::Mmap),
            1324 => Some(Self::NetfilterPkt),
            1327 => Some(Self::Proctitle),
            1100 => Some(Self::UserAuth),
            1101 => Some(Self::UserAcct),
            1102 => Some(Self::UserMgmt),
            1112 => Some(Self::UserLogin),
            1113 => Some(Self::UserLogout),
            1103 => Some(Self::CredAcq),
            1104 => Some(Self::CredDisp),
            1105 => Some(Self::CredRefr),
            1106 => Some(Self::UserCmd),
            1108 => Some(Self::UserSelinuxErr),
            1006 => Some(Self::Login),
            1305 => Some(Self::ConfigChange),
            1700 => Some(Self::Anomaly),
            1800 => Some(Self::Integrity),
            2000 => Some(Self::Kernel),
            1326 => Some(Self::Seccomp),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Syscall => "SYSCALL",
            Self::Path => "PATH",
            Self::Cwd => "CWD",
            Self::Execve => "EXECVE",
            Self::Ipc => "IPC",
            Self::Socketcall => "SOCKETCALL",
            Self::Sockaddr => "SOCKADDR",
            Self::FdPair => "FD_PAIR",
            Self::BprmFcaps => "BPRM_FCAPS",
            Self::Capset => "CAPSET",
            Self::Mmap => "MMAP",
            Self::NetfilterPkt => "NETFILTER_PKT",
            Self::Proctitle => "PROCTITLE",
            Self::UserAuth => "USER_AUTH",
            Self::UserAcct => "USER_ACCT",
            Self::UserMgmt => "USER_MGMT",
            Self::UserLogin => "USER_LOGIN",
            Self::UserLogout => "USER_LOGOUT",
            Self::CredAcq => "CRED_ACQ",
            Self::CredDisp => "CRED_DISP",
            Self::CredRefr => "CRED_REFR",
            Self::UserCmd => "USER_CMD",
            Self::UserTty => "USER_TTY",
            Self::UserAvc => "USER_AVC",
            Self::UserSelinuxErr => "USER_SELINUX_ERR",
            Self::Login => "LOGIN",
            Self::ConfigChange => "CONFIG_CHANGE",
            Self::Anomaly => "ANOMALY",
            Self::Integrity => "INTEGRITY",
            Self::Kernel => "KERNEL",
            Self::Seccomp => "SECCOMP",
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Audit Rule Fields & Operators
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AuditField {
    Pid = 0,
    Uid = 1,
    Gid = 2,
    LoginUid = 3,
    SessionId = 4,
    DevMajor = 5,
    DevMinor = 6,
    Inode = 7,
    Exit = 8,
    Success = 9,
    A0 = 10,
    A1 = 11,
    A2 = 12,
    A3 = 13,
    Arch = 14,
    MsgType = 15,
    SubjUser = 16,
    SubjRole = 17,
    SubjType = 18,
    SubjSen = 19,
    SubjClr = 20,
    ObjUser = 21,
    ObjRole = 22,
    ObjType = 23,
    ObjLevLow = 24,
    ObjLevHigh = 25,
    Watch = 26,
    Dir = 27,
    FilterKey = 28,
    Perm = 29,
    Exe = 30,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditOperator {
    Equal = 0,
    NotEqual = 1,
    GreaterThan = 2,
    LessThan = 3,
    GreaterThanOrEqual = 4,
    LessThanOrEqual = 5,
    BitMask = 6,
    BitTest = 7,
}

impl AuditOperator {
    pub fn evaluate(&self, field_val: u64, rule_val: u64) -> bool {
        match self {
            Self::Equal => field_val == rule_val,
            Self::NotEqual => field_val != rule_val,
            Self::GreaterThan => field_val > rule_val,
            Self::LessThan => field_val < rule_val,
            Self::GreaterThanOrEqual => field_val >= rule_val,
            Self::LessThanOrEqual => field_val <= rule_val,
            Self::BitMask => (field_val & rule_val) != 0,
            Self::BitTest => (field_val & rule_val) == rule_val,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Audit Filter Lists
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditFilterList {
    Task = 0,
    Exit = 1,
    User = 2,
    Exclude = 3,
    Filesystem = 4,
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. Audit Rule Condition & Rule
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AuditCondition {
    pub field: AuditField,
    pub operator: AuditOperator,
    pub value: u64,
    pub str_val: Option<String>,
}

impl AuditCondition {
    pub fn new_numeric(field: AuditField, op: AuditOperator, value: u64) -> Self {
        Self {
            field,
            operator: op,
            value,
            str_val: None,
        }
    }

    pub fn new_string(field: AuditField, op: AuditOperator, s: String) -> Self {
        let hash = Self::hash_str(&s);
        Self {
            field,
            operator: op,
            value: hash,
            str_val: Some(s),
        }
    }

    fn hash_str(s: &str) -> u64 {
        let mut h: u64 = 5381;
        for b in s.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u64);
        }
        h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditAction {
    Never = 0,
    Always = 1,
}

#[derive(Debug, Clone)]
pub struct AuditRule {
    pub filter_list: AuditFilterList,
    pub action: AuditAction,
    pub conditions: Vec<AuditCondition>,
    pub filter_key: Option<String>,
    pub syscall_mask: [u64; 8],
}

impl AuditRule {
    pub fn new(list: AuditFilterList, action: AuditAction) -> Self {
        Self {
            filter_list: list,
            action,
            conditions: Vec::new(),
            filter_key: None,
            syscall_mask: [u64::MAX; 8],
        }
    }

    pub fn add_condition(&mut self, cond: AuditCondition) {
        self.conditions.push(cond);
    }

    pub fn set_filter_key(&mut self, key: String) {
        self.filter_key = Some(key);
    }

    pub fn set_syscall(&mut self, nr: u32) {
        let idx = (nr / 64) as usize;
        let bit = nr % 64;
        if idx < 8 {
            self.syscall_mask = [0; 8];
            self.syscall_mask[idx] |= 1u64 << bit;
        }
    }

    pub fn matches_syscall(&self, nr: u32) -> bool {
        let idx = (nr / 64) as usize;
        let bit = nr % 64;
        if idx < 8 {
            (self.syscall_mask[idx] & (1u64 << bit)) != 0
        } else {
            false
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. Audit Record
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AuditField2 {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub event_type: AuditEventType,
    pub timestamp: u64,
    pub serial: u64,
    pub fields: Vec<AuditField2>,
}

impl AuditRecord {
    pub fn new(event_type: AuditEventType, timestamp: u64, serial: u64) -> Self {
        Self {
            event_type,
            timestamp,
            serial,
            fields: Vec::new(),
        }
    }

    pub fn add_field(&mut self, key: &str, value: &str) {
        self.fields.push(AuditField2 {
            key: String::from(key),
            value: String::from(value),
        });
    }

    pub fn format_raw(&self) -> String {
        use alloc::format;
        let mut s = format!(
            "type={} msg=audit({}.{}:{}): ",
            self.event_type.name(),
            self.timestamp / 1000,
            self.timestamp % 1000,
            self.serial,
        );
        for (i, f) in self.fields.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&f.key);
            s.push('=');
            s.push_str(&f.value);
        }
        s
    }

    pub fn format_enriched(&self) -> String {
        use alloc::format;
        let mut s = format!(
            "type={} msg=audit({}.{}:{}):",
            self.event_type.name(),
            self.timestamp / 1000,
            self.timestamp % 1000,
            self.serial,
        );
        for f in &self.fields {
            s.push_str(" [");
            s.push_str(&f.key);
            s.push_str("]=");
            s.push_str(&f.value);
        }
        s
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. File Watch Permissions
// ───────────────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct WatchPerm: u8 {
        const READ      = 0x01;
        const WRITE     = 0x02;
        const EXECUTE   = 0x04;
        const ATTR      = 0x08;
    }
}

#[derive(Debug, Clone)]
pub struct FileWatch {
    pub path: String,
    pub perm: WatchPerm,
    pub filter_key: Option<String>,
}

impl FileWatch {
    pub fn new(path: &str, perm: WatchPerm) -> Self {
        Self {
            path: String::from(path),
            perm,
            filter_key: None,
        }
    }

    pub fn matches(&self, accessed_path: &str, access: WatchPerm) -> bool {
        if accessed_path != self.path {
            return false;
        }
        self.perm.intersects(access)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. Audit Log Ring Buffer
// ───────────────────────────────────────────────────────────────────────────────

const AUDIT_LOG_CAPACITY: usize = 4096;
const AUDIT_LOG_ROTATE_THRESHOLD: usize = 3584;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditLogFormat {
    Raw,
    Enriched,
}

pub struct AuditLogBuffer {
    entries: Vec<AuditRecord>,
    head: usize,
    tail: usize,
    count: usize,
    capacity: usize,
    format: AuditLogFormat,
    total_written: u64,
    rotations: u32,
}

impl AuditLogBuffer {
    pub fn new(capacity: usize) -> Self {
        let mut entries = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            entries.push(AuditRecord::new(AuditEventType::Kernel, 0, 0));
        }
        Self {
            entries,
            head: 0,
            tail: 0,
            count: 0,
            capacity,
            format: AuditLogFormat::Raw,
            total_written: 0,
            rotations: 0,
        }
    }

    pub fn write(&mut self, record: AuditRecord) {
        if self.count >= self.capacity {
            self.head = (self.head + 1) % self.capacity;
            self.count -= 1;
        }
        self.entries[self.tail] = record;
        self.tail = (self.tail + 1) % self.capacity;
        self.count += 1;
        self.total_written += 1;

        if self.count >= AUDIT_LOG_ROTATE_THRESHOLD {
            self.rotate();
        }
    }

    pub fn read_latest(&self, max: usize) -> Vec<&AuditRecord> {
        let n = max.min(self.count);
        let mut result = Vec::with_capacity(n);
        let start = if self.tail >= n {
            self.tail - n
        } else {
            self.capacity - (n - self.tail)
        };
        for i in 0..n {
            let idx = (start + i) % self.capacity;
            result.push(&self.entries[idx]);
        }
        result
    }

    fn rotate(&mut self) {
        let discard = self.count / 4;
        self.head = (self.head + discard) % self.capacity;
        self.count -= discard;
        self.rotations += 1;
    }

    pub fn set_format(&mut self, fmt: AuditLogFormat) {
        self.format = fmt;
    }

    pub fn total_written(&self) -> u64 {
        self.total_written
    }
    pub fn rotations(&self) -> u32 {
        self.rotations
    }
    pub fn len(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 8. Failure Mode & auditctl Config
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditFailureMode {
    Silent = 0,
    Printk = 1,
    Panic = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditConfig {
    pub enabled: bool,
    pub backlog_limit: u32,
    pub failure_mode: AuditFailureMode,
    pub rate_limit: u32,
    pub backlog_wait: u32,
    pub pid: u32,
}

impl AuditConfig {
    pub const fn default_config() -> Self {
        Self {
            enabled: true,
            backlog_limit: 8192,
            failure_mode: AuditFailureMode::Printk,
            rate_limit: 0,
            backlog_wait: 60000,
            pid: 0,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 9. Audit Session Tracking
// ───────────────────────────────────────────────────────────────────────────────

pub const AUDIT_LOGINUID_UNSET: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone)]
pub struct AuditSession {
    pub session_id: u32,
    pub login_uid: u32,
    pub pid: u32,
    pub tty: Option<String>,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub hostname: Option<String>,
    pub addr: Option<String>,
}

impl AuditSession {
    pub fn new(session_id: u32, login_uid: u32, pid: u32, time: u64) -> Self {
        Self {
            session_id,
            login_uid,
            pid,
            tty: None,
            start_time: time,
            end_time: None,
            hostname: None,
            addr: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.end_time.is_none()
    }

    pub fn close(&mut self, time: u64) {
        self.end_time = Some(time);
    }
}

pub struct SessionTracker {
    sessions: BTreeMap<u32, AuditSession>,
    next_session_id: u32,
    loginuid_map: BTreeMap<u32, u32>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            next_session_id: 1,
            loginuid_map: BTreeMap::new(),
        }
    }

    pub fn create_session(&mut self, login_uid: u32, pid: u32, time: u64) -> u32 {
        let sid = self.next_session_id;
        self.next_session_id += 1;
        let session = AuditSession::new(sid, login_uid, pid, time);
        self.sessions.insert(sid, session);
        self.loginuid_map.insert(pid, login_uid);
        sid
    }

    pub fn close_session(&mut self, session_id: u32, time: u64) {
        if let Some(sess) = self.sessions.get_mut(&session_id) {
            sess.close(time);
        }
    }

    pub fn get_login_uid(&self, pid: u32) -> u32 {
        self.loginuid_map
            .get(&pid)
            .copied()
            .unwrap_or(AUDIT_LOGINUID_UNSET)
    }

    pub fn get_session(&self, sid: u32) -> Option<&AuditSession> {
        self.sessions.get(&sid)
    }

    pub fn active_sessions(&self) -> usize {
        self.sessions.values().filter(|s| s.is_active()).count()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 10. Audit Daemon Communication (netlink-based)
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuditNetlinkMsgType {
    GetStatus = 0,
    SetStatus = 1,
    ListRules = 2,
    AddRule = 3,
    DelRule = 4,
    UserMsg = 5,
    LoginMsg = 6,
    WatchIns = 7,
    WatchRem = 8,
    Signal = 9,
    MakeEquiv = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatcherMode {
    Unicast,
    Multicast,
}

pub struct AuditNetlinkChannel {
    pub dispatcher_mode: DispatcherMode,
    pub daemon_pid: Option<u32>,
    pub pending_msgs: Vec<AuditNetlinkMessage>,
    pub max_queue: usize,
}

#[derive(Debug, Clone)]
pub struct AuditNetlinkMessage {
    pub msg_type: AuditNetlinkMsgType,
    pub seq: u32,
    pub payload: Vec<u8>,
}

impl AuditNetlinkChannel {
    pub fn new() -> Self {
        Self {
            dispatcher_mode: DispatcherMode::Unicast,
            daemon_pid: None,
            pending_msgs: Vec::new(),
            max_queue: 1024,
        }
    }

    pub fn register_daemon(&mut self, pid: u32) {
        self.daemon_pid = Some(pid);
    }

    pub fn unregister_daemon(&mut self) {
        self.daemon_pid = None;
    }

    pub fn send_to_daemon(&mut self, msg: AuditNetlinkMessage) -> bool {
        if self.daemon_pid.is_none() {
            return false;
        }
        if self.pending_msgs.len() >= self.max_queue {
            self.pending_msgs.remove(0);
        }
        self.pending_msgs.push(msg);
        true
    }

    pub fn drain_pending(&mut self) -> Vec<AuditNetlinkMessage> {
        core::mem::take(&mut self.pending_msgs)
    }

    pub fn set_mode(&mut self, mode: DispatcherMode) {
        self.dispatcher_mode = mode;
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 11. Integrity Measurement Architecture (IMA)
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImaAction {
    Measure,
    DontMeasure,
    Appraise,
    DontAppraise,
    Audit,
    Hash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImaHook {
    FileCheck,
    MmapCheck,
    BprmCheck,
    ModuleCheck,
    FirmwareCheck,
    PolicyCheck,
    KexecKernelCheck,
}

#[derive(Debug, Clone)]
pub struct ImaRule {
    pub action: ImaAction,
    pub hook: ImaHook,
    pub uid: Option<u32>,
    pub fowner: Option<u32>,
    pub mask: Option<WatchPerm>,
}

#[derive(Debug, Clone)]
pub struct ImaMeasurement {
    pub pcr: u8,
    pub template: String,
    pub hash: [u8; 32],
    pub file_path: String,
    pub timestamp: u64,
}

pub struct IntegritySubsystem {
    pub policy: Vec<ImaRule>,
    pub measurements: Vec<ImaMeasurement>,
    pub violations: u64,
    pub pcr_extend: [u8; 32],
}

impl IntegritySubsystem {
    pub fn new() -> Self {
        Self {
            policy: Vec::new(),
            measurements: Vec::new(),
            violations: 0,
            pcr_extend: [0u8; 32],
        }
    }

    pub fn add_rule(&mut self, rule: ImaRule) {
        self.policy.push(rule);
    }

    pub fn measure_file(&mut self, path: &str, hash: [u8; 32], time: u64) {
        let entry = ImaMeasurement {
            pcr: 10,
            template: String::from("ima-ng"),
            hash,
            file_path: String::from(path),
            timestamp: time,
        };

        for i in 0..32 {
            self.pcr_extend[i] ^= hash[i];
        }
        self.measurements.push(entry);
    }

    pub fn verify_file(&self, path: &str, expected: &[u8; 32]) -> bool {
        for m in self.measurements.iter().rev() {
            if m.file_path == path {
                // Constant-time integrity-hash comparison (defense-in-depth).
                return crate::crypto::ct_eq(&m.hash, expected);
            }
        }
        false
    }

    pub fn appraise_file(&mut self, path: &str, hash: [u8; 32], expected: &[u8; 32]) -> bool {
        if hash == *expected {
            true
        } else {
            self.violations += 1;
            false
        }
    }

    pub fn measurement_count(&self) -> usize {
        self.measurements.len()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 12. Syscall Context (for building audit records from syscall events)
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SyscallContext {
    pub arch: u32,
    pub syscall_nr: u32,
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
    pub exit_code: i64,
    pub success: bool,
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    pub fsuid: u32,
    pub fsgid: u32,
    pub ppid: u32,
    pub comm: String,
    pub exe: String,
    pub tty: Option<String>,
    pub ses: u32,
    pub auid: u32,
}

impl SyscallContext {
    pub fn field_value(&self, field: AuditField) -> u64 {
        match field {
            AuditField::Pid => self.pid as u64,
            AuditField::Uid => self.uid as u64,
            AuditField::Gid => self.gid as u64,
            AuditField::LoginUid => self.auid as u64,
            AuditField::SessionId => self.ses as u64,
            AuditField::Exit => self.exit_code as u64,
            AuditField::Success => {
                if self.success {
                    1
                } else {
                    0
                }
            }
            AuditField::A0 => self.a0,
            AuditField::A1 => self.a1,
            AuditField::A2 => self.a2,
            AuditField::A3 => self.a3,
            AuditField::Arch => self.arch as u64,
            _ => 0,
        }
    }

    pub fn to_record(&self, serial: u64, timestamp: u64) -> AuditRecord {
        use alloc::format;
        let mut rec = AuditRecord::new(AuditEventType::Syscall, timestamp, serial);
        rec.add_field("arch", &format!("{:#x}", self.arch));
        rec.add_field("syscall", &format!("{}", self.syscall_nr));
        rec.add_field("success", if self.success { "yes" } else { "no" });
        rec.add_field("exit", &format!("{}", self.exit_code));
        rec.add_field("a0", &format!("{:#x}", self.a0));
        rec.add_field("a1", &format!("{:#x}", self.a1));
        rec.add_field("a2", &format!("{:#x}", self.a2));
        rec.add_field("a3", &format!("{:#x}", self.a3));
        rec.add_field("ppid", &format!("{}", self.ppid));
        rec.add_field("pid", &format!("{}", self.pid));
        rec.add_field("auid", &format!("{}", self.auid));
        rec.add_field("uid", &format!("{}", self.uid));
        rec.add_field("gid", &format!("{}", self.gid));
        rec.add_field("euid", &format!("{}", self.euid));
        rec.add_field("egid", &format!("{}", self.egid));
        rec.add_field("suid", &format!("{}", self.suid));
        rec.add_field("sgid", &format!("{}", self.sgid));
        rec.add_field("fsuid", &format!("{}", self.fsuid));
        rec.add_field("fsgid", &format!("{}", self.fsgid));
        rec.add_field("ses", &format!("{}", self.ses));
        rec.add_field("comm", &self.comm);
        rec.add_field("exe", &self.exe);
        if let Some(ref tty) = self.tty {
            rec.add_field("tty", tty);
        }
        rec
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 13. Rule Evaluation Engine
// ───────────────────────────────────────────────────────────────────────────────

pub struct RuleEngine {
    pub task_rules: Vec<AuditRule>,
    pub exit_rules: Vec<AuditRule>,
    pub user_rules: Vec<AuditRule>,
    pub exclude_rules: Vec<AuditRule>,
    pub filesystem_rules: Vec<AuditRule>,
    pub file_watches: Vec<FileWatch>,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self {
            task_rules: Vec::new(),
            exit_rules: Vec::new(),
            user_rules: Vec::new(),
            exclude_rules: Vec::new(),
            filesystem_rules: Vec::new(),
            file_watches: Vec::new(),
        }
    }

    pub fn add_rule(&mut self, rule: AuditRule) {
        match rule.filter_list {
            AuditFilterList::Task => self.task_rules.push(rule),
            AuditFilterList::Exit => self.exit_rules.push(rule),
            AuditFilterList::User => self.user_rules.push(rule),
            AuditFilterList::Exclude => self.exclude_rules.push(rule),
            AuditFilterList::Filesystem => self.filesystem_rules.push(rule),
        }
    }

    pub fn delete_rule(&mut self, list: AuditFilterList, index: usize) -> bool {
        let rules = match list {
            AuditFilterList::Task => &mut self.task_rules,
            AuditFilterList::Exit => &mut self.exit_rules,
            AuditFilterList::User => &mut self.user_rules,
            AuditFilterList::Exclude => &mut self.exclude_rules,
            AuditFilterList::Filesystem => &mut self.filesystem_rules,
        };
        if index < rules.len() {
            rules.remove(index);
            true
        } else {
            false
        }
    }

    pub fn clear_rules(&mut self, list: AuditFilterList) {
        match list {
            AuditFilterList::Task => self.task_rules.clear(),
            AuditFilterList::Exit => self.exit_rules.clear(),
            AuditFilterList::User => self.user_rules.clear(),
            AuditFilterList::Exclude => self.exclude_rules.clear(),
            AuditFilterList::Filesystem => self.filesystem_rules.clear(),
        }
    }

    pub fn list_rules(&self, list: AuditFilterList) -> &[AuditRule] {
        match list {
            AuditFilterList::Task => &self.task_rules,
            AuditFilterList::Exit => &self.exit_rules,
            AuditFilterList::User => &self.user_rules,
            AuditFilterList::Exclude => &self.exclude_rules,
            AuditFilterList::Filesystem => &self.filesystem_rules,
        }
    }

    pub fn total_rules(&self) -> usize {
        self.task_rules.len()
            + self.exit_rules.len()
            + self.user_rules.len()
            + self.exclude_rules.len()
            + self.filesystem_rules.len()
            + self.file_watches.len()
    }

    pub fn evaluate_exit(&self, ctx: &SyscallContext) -> Option<AuditAction> {
        for rule in &self.exit_rules {
            if !rule.matches_syscall(ctx.syscall_nr) {
                continue;
            }
            let mut all_match = true;
            for cond in &rule.conditions {
                let fv = ctx.field_value(cond.field);
                if !cond.operator.evaluate(fv, cond.value) {
                    all_match = false;
                    break;
                }
            }
            if all_match {
                return Some(rule.action);
            }
        }
        None
    }

    pub fn is_excluded(&self, event_type: AuditEventType) -> bool {
        for rule in &self.exclude_rules {
            for cond in &rule.conditions {
                if cond.field == AuditField::MsgType
                    && cond.operator == AuditOperator::Equal
                    && cond.value == event_type as u64
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn add_file_watch(&mut self, watch: FileWatch) {
        self.file_watches.push(watch);
    }

    pub fn check_file_watch(&self, path: &str, access: WatchPerm) -> Option<&FileWatch> {
        self.file_watches.iter().find(|w| w.matches(path, access))
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 14. Global Audit System
// ───────────────────────────────────────────────────────────────────────────────

pub struct AuditSystem {
    pub config: AuditConfig,
    pub rules: RuleEngine,
    pub log: AuditLogBuffer,
    pub sessions: SessionTracker,
    pub netlink: AuditNetlinkChannel,
    pub integrity: IntegritySubsystem,
    pub serial: u64,
    pub backlog: u32,
    pub lost: u64,
    pub rate_count: u32,
    pub rate_epoch: u64,
}

impl AuditSystem {
    pub fn new() -> Self {
        Self {
            config: AuditConfig::default_config(),
            rules: RuleEngine::new(),
            log: AuditLogBuffer::new(AUDIT_LOG_CAPACITY),
            sessions: SessionTracker::new(),
            netlink: AuditNetlinkChannel::new(),
            integrity: IntegritySubsystem::new(),
            serial: 1,
            backlog: 0,
            lost: 0,
            rate_count: 0,
            rate_epoch: 0,
        }
    }

    fn next_serial(&mut self) -> u64 {
        let s = self.serial;
        self.serial += 1;
        s
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.config.enabled = on;
    }

    pub fn set_backlog_limit(&mut self, limit: u32) {
        self.config.backlog_limit = limit;
    }

    pub fn set_failure_mode(&mut self, mode: AuditFailureMode) {
        self.config.failure_mode = mode;
    }

    pub fn set_rate_limit(&mut self, rate: u32) {
        self.config.rate_limit = rate;
    }

    fn check_rate_limit(&mut self, now: u64) -> bool {
        if self.config.rate_limit == 0 {
            return true;
        }
        if now != self.rate_epoch {
            self.rate_epoch = now;
            self.rate_count = 0;
        }
        self.rate_count += 1;
        self.rate_count <= self.config.rate_limit
    }

    fn handle_backlog_overflow(&mut self) {
        match self.config.failure_mode {
            AuditFailureMode::Silent => {
                self.lost += 1;
            }
            AuditFailureMode::Printk => {
                self.lost += 1;
            }
            AuditFailureMode::Panic => {
                panic!("audit: backlog limit exceeded");
            }
        }
    }

    pub fn log_syscall(&mut self, ctx: &SyscallContext, timestamp: u64) {
        if !self.config.enabled {
            return;
        }
        if self.rules.is_excluded(AuditEventType::Syscall) {
            return;
        }
        if !self.check_rate_limit(timestamp / 1000) {
            self.lost += 1;
            return;
        }
        let action = self.rules.evaluate_exit(ctx);
        match action {
            Some(AuditAction::Never) => return,
            Some(AuditAction::Always) | None => {}
        }
        if self.backlog >= self.config.backlog_limit {
            self.handle_backlog_overflow();
            return;
        }
        let serial = self.next_serial();
        let record = ctx.to_record(serial, timestamp);

        self.netlink.send_to_daemon(AuditNetlinkMessage {
            msg_type: AuditNetlinkMsgType::UserMsg,
            seq: serial as u32,
            payload: Vec::new(),
        });

        self.log.write(record);
        self.backlog += 1;
    }

    pub fn log_event(
        &mut self,
        event_type: AuditEventType,
        timestamp: u64,
        fields: Vec<(&str, &str)>,
    ) {
        if !self.config.enabled {
            return;
        }
        if self.rules.is_excluded(event_type) {
            return;
        }
        let serial = self.next_serial();
        let mut record = AuditRecord::new(event_type, timestamp, serial);
        for (k, v) in fields {
            record.add_field(k, v);
        }
        self.log.write(record);
    }

    pub fn log_file_access(
        &mut self,
        path: &str,
        access: WatchPerm,
        pid: u32,
        uid: u32,
        timestamp: u64,
    ) {
        if !self.config.enabled {
            return;
        }
        let matched = self.rules.check_file_watch(path, access).is_some();
        if !matched {
            return;
        }
        use alloc::format;
        let serial = self.next_serial();
        let mut record = AuditRecord::new(AuditEventType::Path, timestamp, serial);
        record.add_field("name", path);
        record.add_field("pid", &format!("{}", pid));
        record.add_field("uid", &format!("{}", uid));
        record.add_field("perm", &format!("{:?}", access));
        self.log.write(record);
    }

    pub fn login_event(&mut self, pid: u32, uid: u32, login_uid: u32, timestamp: u64) -> u32 {
        let sid = self.sessions.create_session(login_uid, pid, timestamp);
        use alloc::format;
        self.log_event(
            AuditEventType::Login,
            timestamp,
            alloc::vec![
                ("pid", &format!("{}", pid)),
                ("uid", &format!("{}", uid)),
                ("auid", &format!("{}", login_uid)),
                ("ses", &format!("{}", sid)),
            ],
        );
        sid
    }

    pub fn logout_event(&mut self, session_id: u32, timestamp: u64) {
        self.sessions.close_session(session_id, timestamp);
        use alloc::format;
        self.log_event(
            AuditEventType::UserLogout,
            timestamp,
            alloc::vec![("ses", &format!("{}", session_id)),],
        );
    }

    pub fn config_change(&mut self, key: &str, old_val: &str, new_val: &str, timestamp: u64) {
        self.log_event(
            AuditEventType::ConfigChange,
            timestamp,
            alloc::vec![
                ("op", "set"),
                ("key", key),
                ("old", old_val),
                ("new", new_val),
            ],
        );
    }

    pub fn stats(&self) -> AuditStats {
        AuditStats {
            enabled: self.config.enabled,
            total_rules: self.rules.total_rules(),
            total_logged: self.log.total_written(),
            lost: self.lost,
            backlog: self.backlog,
            backlog_limit: self.config.backlog_limit,
            active_sessions: self.sessions.active_sessions(),
            ima_measurements: self.integrity.measurement_count(),
        }
    }
}

#[derive(Debug)]
pub struct AuditStats {
    pub enabled: bool,
    pub total_rules: usize,
    pub total_logged: u64,
    pub lost: u64,
    pub backlog: u32,
    pub backlog_limit: u32,
    pub active_sessions: usize,
    pub ima_measurements: usize,
}

pub static AUDIT_SYSTEM: Mutex<Option<AuditSystem>> = Mutex::new(None);

pub fn init() {
    let mut sys = AUDIT_SYSTEM.lock();
    *sys = Some(AuditSystem::new());
}
