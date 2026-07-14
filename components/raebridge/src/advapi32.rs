//! advapi32.dll — Registry, security, cryptography, event logging, and
//! service control manager APIs for AthBridge.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    CompatContext, HandleType, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
};

// =========================================================================
// Predefined registry key handles
// =========================================================================

pub const HKEY_CLASSES_ROOT: u64 = 0x80000000;
pub const HKEY_CURRENT_USER: u64 = 0x80000001;
pub const HKEY_LOCAL_MACHINE: u64 = 0x80000002;
pub const HKEY_USERS: u64 = 0x80000003;
pub const HKEY_CURRENT_CONFIG: u64 = 0x80000005;
pub const HKEY_PERFORMANCE_DATA: u64 = 0x80000004;

// =========================================================================
// Registry value types
// =========================================================================

pub const REG_NONE: u32 = 0;
pub const REG_SZ: u32 = 1;
pub const REG_EXPAND_SZ: u32 = 2;
pub const REG_BINARY: u32 = 3;
pub const REG_DWORD: u32 = 4;
pub const REG_DWORD_BIG_ENDIAN: u32 = 5;
pub const REG_LINK: u32 = 6;
pub const REG_MULTI_SZ: u32 = 7;
pub const REG_QWORD: u32 = 11;

// =========================================================================
// Registry access rights
// =========================================================================

pub const KEY_QUERY_VALUE: u32 = 0x0001;
pub const KEY_SET_VALUE: u32 = 0x0002;
pub const KEY_CREATE_SUB_KEY: u32 = 0x0004;
pub const KEY_ENUMERATE_SUB_KEYS: u32 = 0x0008;
pub const KEY_NOTIFY: u32 = 0x0010;
pub const KEY_CREATE_LINK: u32 = 0x0020;
pub const KEY_READ: u32 = 0x20019;
pub const KEY_WRITE: u32 = 0x20006;
pub const KEY_ALL_ACCESS: u32 = 0xF003F;

// =========================================================================
// Registry disposition values
// =========================================================================

pub const REG_CREATED_NEW_KEY: u32 = 0x00000001;
pub const REG_OPENED_EXISTING_KEY: u32 = 0x00000002;

// =========================================================================
// Registry notification filter flags
// =========================================================================

pub const REG_NOTIFY_CHANGE_NAME: u32 = 0x00000001;
pub const REG_NOTIFY_CHANGE_ATTRIBUTES: u32 = 0x00000002;
pub const REG_NOTIFY_CHANGE_LAST_SET: u32 = 0x00000004;
pub const REG_NOTIFY_CHANGE_SECURITY: u32 = 0x00000008;

// =========================================================================
// Security token access rights
// =========================================================================

pub const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
pub const TOKEN_DUPLICATE: u32 = 0x0002;
pub const TOKEN_IMPERSONATE: u32 = 0x0004;
pub const TOKEN_QUERY: u32 = 0x0008;
pub const TOKEN_QUERY_SOURCE: u32 = 0x0010;
pub const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
pub const TOKEN_ADJUST_GROUPS: u32 = 0x0040;
pub const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
pub const TOKEN_ALL_ACCESS: u32 = 0xF01FF;

// =========================================================================
// Token information classes
// =========================================================================

pub const TOKEN_INFO_USER: u32 = 1;
pub const TOKEN_INFO_GROUPS: u32 = 2;
pub const TOKEN_INFO_PRIVILEGES: u32 = 3;
pub const TOKEN_INFO_OWNER: u32 = 4;
pub const TOKEN_INFO_TYPE: u32 = 8;
pub const TOKEN_INFO_ELEVATION: u32 = 20;

// =========================================================================
// Privilege attributes
// =========================================================================

pub const SE_PRIVILEGE_ENABLED: u32 = 0x00000002;
pub const SE_PRIVILEGE_REMOVED: u32 = 0x00000004;
pub const SE_PRIVILEGE_ENABLED_BY_DEFAULT: u32 = 0x00000001;

// =========================================================================
// Security information flags
// =========================================================================

pub const OWNER_SECURITY_INFORMATION: u32 = 0x00000001;
pub const GROUP_SECURITY_INFORMATION: u32 = 0x00000002;
pub const DACL_SECURITY_INFORMATION: u32 = 0x00000004;
pub const SACL_SECURITY_INFORMATION: u32 = 0x00000008;

pub const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

// =========================================================================
// SE object types
// =========================================================================

pub const SE_UNKNOWN_OBJECT_TYPE: u32 = 0;
pub const SE_FILE_OBJECT: u32 = 1;
pub const SE_SERVICE: u32 = 2;
pub const SE_PRINTER: u32 = 3;
pub const SE_REGISTRY_KEY: u32 = 4;
pub const SE_KERNEL_OBJECT: u32 = 6;

// =========================================================================
// SID use types
// =========================================================================

pub const SID_TYPE_USER: u32 = 1;
pub const SID_TYPE_GROUP: u32 = 2;
pub const SID_TYPE_DOMAIN: u32 = 3;
pub const SID_TYPE_ALIAS: u32 = 4;
pub const SID_TYPE_WELL_KNOWN_GROUP: u32 = 5;

// =========================================================================
// Crypto provider types
// =========================================================================

pub const PROV_RSA_FULL: u32 = 1;
pub const PROV_RSA_SIG: u32 = 2;
pub const PROV_RSA_AES: u32 = 24;
pub const CRYPT_VERIFYCONTEXT: u32 = 0xF0000000;
pub const CRYPT_NEWKEYSET: u32 = 0x00000008;

// =========================================================================
// Crypto algorithm IDs
// =========================================================================

pub const CALG_MD5: u32 = 0x00008003;
pub const CALG_SHA1: u32 = 0x00008004;
pub const CALG_SHA_256: u32 = 0x0000800C;
pub const CALG_SHA_384: u32 = 0x0000800D;
pub const CALG_SHA_512: u32 = 0x0000800E;
pub const CALG_AES_128: u32 = 0x0000660E;
pub const CALG_AES_256: u32 = 0x00006610;
pub const CALG_RC4: u32 = 0x00006801;

// Hash parameter IDs
pub const HP_HASHVAL: u32 = 0x0002;
pub const HP_HASHSIZE: u32 = 0x0004;

// =========================================================================
// Event log types
// =========================================================================

pub const EVENTLOG_SUCCESS: u16 = 0x0000;
pub const EVENTLOG_ERROR_TYPE: u16 = 0x0001;
pub const EVENTLOG_WARNING_TYPE: u16 = 0x0002;
pub const EVENTLOG_INFORMATION_TYPE: u16 = 0x0004;
pub const EVENTLOG_AUDIT_SUCCESS: u16 = 0x0008;
pub const EVENTLOG_AUDIT_FAILURE: u16 = 0x0010;

pub const EVENTLOG_SEQUENTIAL_READ: u32 = 0x0001;
pub const EVENTLOG_SEEK_READ: u32 = 0x0002;
pub const EVENTLOG_FORWARDS_READ: u32 = 0x0004;
pub const EVENTLOG_BACKWARDS_READ: u32 = 0x0008;

// =========================================================================
// Service control manager constants
// =========================================================================

pub const SC_MANAGER_ALL_ACCESS: u32 = 0xF003F;
pub const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
pub const SC_MANAGER_CONNECT: u32 = 0x0001;
pub const SC_MANAGER_ENUMERATE_SERVICE: u32 = 0x0004;

pub const SERVICE_ALL_ACCESS: u32 = 0xF01FF;
pub const SERVICE_START: u32 = 0x0010;
pub const SERVICE_STOP: u32 = 0x0020;
pub const SERVICE_QUERY_STATUS: u32 = 0x0004;

pub const SERVICE_WIN32_OWN_PROCESS: u32 = 0x00000010;
pub const SERVICE_WIN32_SHARE_PROCESS: u32 = 0x00000020;
pub const SERVICE_KERNEL_DRIVER: u32 = 0x00000001;
pub const SERVICE_FILE_SYSTEM_DRIVER: u32 = 0x00000002;
pub const SERVICE_INTERACTIVE_PROCESS: u32 = 0x00000100;

pub const SERVICE_AUTO_START: u32 = 0x00000002;
pub const SERVICE_BOOT_START: u32 = 0x00000000;
pub const SERVICE_DEMAND_START: u32 = 0x00000003;
pub const SERVICE_DISABLED: u32 = 0x00000004;
pub const SERVICE_SYSTEM_START: u32 = 0x00000001;

pub const SERVICE_ERROR_IGNORE: u32 = 0x00000000;
pub const SERVICE_ERROR_NORMAL: u32 = 0x00000001;
pub const SERVICE_ERROR_SEVERE: u32 = 0x00000002;
pub const SERVICE_ERROR_CRITICAL: u32 = 0x00000003;

pub const SERVICE_STOPPED: u32 = 0x00000001;
pub const SERVICE_START_PENDING: u32 = 0x00000002;
pub const SERVICE_STOP_PENDING: u32 = 0x00000003;
pub const SERVICE_RUNNING: u32 = 0x00000004;
pub const SERVICE_CONTINUE_PENDING: u32 = 0x00000005;
pub const SERVICE_PAUSE_PENDING: u32 = 0x00000006;
pub const SERVICE_PAUSED: u32 = 0x00000007;

pub const SERVICE_CONTROL_STOP: u32 = 0x00000001;
pub const SERVICE_CONTROL_PAUSE: u32 = 0x00000002;
pub const SERVICE_CONTROL_CONTINUE: u32 = 0x00000003;
pub const SERVICE_CONTROL_INTERROGATE: u32 = 0x00000004;

pub const SERVICE_ACCEPT_STOP: u32 = 0x00000001;
pub const SERVICE_ACCEPT_PAUSE_CONTINUE: u32 = 0x00000002;
pub const SERVICE_ACCEPT_SHUTDOWN: u32 = 0x00000004;

pub const SERVICE_NO_CHANGE: u32 = 0xFFFFFFFF;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct LuidAndAttributes {
    pub luid: u64,
    pub attributes: u32,
}

#[derive(Debug, Clone)]
pub struct TokenPrivileges {
    pub count: u32,
    pub privileges: Vec<LuidAndAttributes>,
}

#[derive(Debug, Clone)]
pub struct EventLogRecord {
    pub record_number: u32,
    pub time_generated: u32,
    pub time_written: u32,
    pub event_id: u32,
    pub event_type: u16,
    pub category: u16,
    pub source: String,
    pub strings: Vec<String>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ServiceStatus {
    pub service_type: u32,
    pub current_state: u32,
    pub controls_accepted: u32,
    pub exit_code: u32,
    pub service_specific_exit: u32,
    pub check_point: u32,
    pub wait_hint: u32,
}

impl ServiceStatus {
    pub fn stopped() -> Self {
        Self {
            service_type: SERVICE_WIN32_OWN_PROCESS,
            current_state: SERVICE_STOPPED,
            controls_accepted: 0,
            exit_code: 0,
            service_specific_exit: 0,
            check_point: 0,
            wait_hint: 0,
        }
    }

    pub fn running() -> Self {
        Self {
            service_type: SERVICE_WIN32_OWN_PROCESS,
            current_state: SERVICE_RUNNING,
            controls_accepted: SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_PAUSE_CONTINUE,
            exit_code: 0,
            service_specific_exit: 0,
            check_point: 0,
            wait_hint: 0,
        }
    }
}

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

fn hkey_to_prefix(hkey: u64) -> Option<&'static str> {
    match hkey {
        HKEY_CLASSES_ROOT => Some("HKCR"),
        HKEY_CURRENT_USER => Some("HKCU"),
        HKEY_LOCAL_MACHINE => Some("HKLM"),
        HKEY_USERS => Some("HKU"),
        HKEY_CURRENT_CONFIG => Some("HKCC"),
        HKEY_PERFORMANCE_DATA => Some("HKPD"),
        _ => None,
    }
}

fn resolve_key_path(ctx: &CompatContext, hkey: u64, sub_key: &str) -> Option<String> {
    if let Some(prefix) = hkey_to_prefix(hkey) {
        let mut path = String::from(prefix);
        if !sub_key.is_empty() {
            path.push('\\');
            path.push_str(sub_key);
        }
        return Some(path);
    }
    if let Some(handle) = ctx.handle_table.get(hkey) {
        if let Some(ref name) = handle.name {
            let mut path = name.clone();
            if !sub_key.is_empty() {
                path.push('\\');
                path.push_str(sub_key);
            }
            return Some(path);
        }
    }
    None
}

// =========================================================================
// Registry API
// =========================================================================

pub fn reg_open_key_ex_w(
    ctx: &mut CompatContext,
    hkey: u64,
    sub_key: &str,
    _options: u32,
    desired: u32,
    result: &mut u64,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, sub_key) {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    if !ctx.registry.key_exists(&path) {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return ERROR_FILE_NOT_FOUND as i32;
    }

    let handle = ctx
        .handle_table
        .allocate(HandleType::RegKey, desired, Some(path));
    *result = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_close_key(ctx: &mut CompatContext, hkey: u64) -> i32 {
    if hkey_to_prefix(hkey).is_some() {
        return ERROR_SUCCESS as i32;
    }

    if ctx.handle_table.close(hkey) {
        set_last_error(ctx, ERROR_SUCCESS);
        ERROR_SUCCESS as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        ERROR_INVALID_HANDLE as i32
    }
}

pub fn reg_create_key_ex_w(
    ctx: &mut CompatContext,
    hkey: u64,
    sub_key: &str,
    _reserved: u32,
    _class: Option<&str>,
    _options: u32,
    desired: u32,
    _security: u64,
    result: &mut u64,
    disposition: &mut u32,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, sub_key) {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    if ctx.registry.key_exists(&path) {
        *disposition = REG_OPENED_EXISTING_KEY;
    } else {
        ctx.registry
            .set_value(&path, "", crate::RegValue::String(String::new()));
        *disposition = REG_CREATED_NEW_KEY;
    }

    let handle = ctx
        .handle_table
        .allocate(HandleType::RegKey, desired, Some(path));
    *result = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_delete_key_w(ctx: &mut CompatContext, hkey: u64, sub_key: &str) -> i32 {
    let path = match resolve_key_path(ctx, hkey, sub_key) {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    if !ctx.registry.key_exists(&path) {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return ERROR_FILE_NOT_FOUND as i32;
    }

    ctx.registry.delete_value(&path, "");
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_delete_value_w(ctx: &mut CompatContext, hkey: u64, value_name: &str) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    if ctx.registry.delete_value(&path, value_name) {
        set_last_error(ctx, ERROR_SUCCESS);
        ERROR_SUCCESS as i32
    } else {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        ERROR_FILE_NOT_FOUND as i32
    }
}

pub fn reg_set_value_ex_w(
    ctx: &mut CompatContext,
    hkey: u64,
    value_name: &str,
    _reserved: u32,
    reg_type: u32,
    data: &[u8],
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    let value = match reg_type {
        REG_SZ | REG_EXPAND_SZ => {
            let wide: Vec<u16> = data
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let s = crate::wide_to_string(&wide);
            if reg_type == REG_EXPAND_SZ {
                crate::RegValue::ExpandString(s)
            } else {
                crate::RegValue::String(s)
            }
        }
        REG_DWORD => {
            if data.len() >= 4 {
                let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                crate::RegValue::DWord(v)
            } else {
                set_last_error(ctx, ERROR_INVALID_PARAMETER);
                return ERROR_INVALID_PARAMETER as i32;
            }
        }
        REG_QWORD => {
            if data.len() >= 8 {
                let v = u64::from_le_bytes([
                    data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
                ]);
                crate::RegValue::QWord(v)
            } else {
                set_last_error(ctx, ERROR_INVALID_PARAMETER);
                return ERROR_INVALID_PARAMETER as i32;
            }
        }
        REG_MULTI_SZ => {
            let wide: Vec<u16> = data
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let full = crate::wide_to_string(&wide);
            let strings: Vec<String> = full.split('\0').map(String::from).collect();
            crate::RegValue::MultiString(strings)
        }
        _ => crate::RegValue::Binary(Vec::from(data)),
    };

    ctx.registry.set_value(&path, value_name, value);
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_query_value_ex_w(
    ctx: &mut CompatContext,
    hkey: u64,
    value_name: &str,
    _reserved: u64,
    reg_type: &mut u32,
    data: &mut [u8],
    data_size: &mut u32,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    let value = match ctx.registry.get_value(&path, value_name) {
        Some(v) => v,
        None => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            return ERROR_FILE_NOT_FOUND as i32;
        }
    };

    let (vtype, raw) = reg_value_to_bytes(&value);
    *reg_type = vtype;

    if (*data_size as usize) < raw.len() {
        *data_size = raw.len() as u32;
        set_last_error(ctx, ERROR_MORE_DATA);
        return ERROR_MORE_DATA as i32;
    }

    let copy_len = raw.len().min(data.len());
    data[..copy_len].copy_from_slice(&raw[..copy_len]);
    *data_size = raw.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

fn reg_value_to_bytes(value: &crate::RegValue) -> (u32, Vec<u8>) {
    match value {
        crate::RegValue::String(s) => {
            let wide = crate::string_to_wide(s);
            let bytes: Vec<u8> = wide.iter().flat_map(|w| w.to_le_bytes()).collect();
            (REG_SZ, bytes)
        }
        crate::RegValue::ExpandString(s) => {
            let wide = crate::string_to_wide(s);
            let bytes: Vec<u8> = wide.iter().flat_map(|w| w.to_le_bytes()).collect();
            (REG_EXPAND_SZ, bytes)
        }
        crate::RegValue::DWord(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
        crate::RegValue::QWord(v) => (REG_QWORD, v.to_le_bytes().to_vec()),
        crate::RegValue::Binary(b) => (REG_BINARY, b.clone()),
        crate::RegValue::MultiString(ss) => {
            let joined = ss.join("\0");
            let wide = crate::string_to_wide(&joined);
            let bytes: Vec<u8> = wide.iter().flat_map(|w| w.to_le_bytes()).collect();
            (REG_MULTI_SZ, bytes)
        }
    }
}

pub fn reg_enum_key_ex_w(
    ctx: &mut CompatContext,
    hkey: u64,
    index: u32,
    name: &mut [u16],
    name_size: &mut u32,
    _class: Option<&mut [u16]>,
    _class_size: Option<&mut u32>,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    let prefix = {
        let mut p = path.clone();
        p.push('\\');
        p
    };

    let mut subkeys: Vec<String> = Vec::new();
    for key in ctx.registry.enumerate_keys() {
        if let Some(rest) = key.strip_prefix(prefix.as_str()) {
            if !rest.contains('\\') && !rest.is_empty() {
                subkeys.push(String::from(rest));
            }
        }
    }
    subkeys.sort();
    subkeys.dedup();

    if (index as usize) >= subkeys.len() {
        set_last_error(ctx, ERROR_NO_MORE_ITEMS);
        return ERROR_NO_MORE_ITEMS as i32;
    }

    let key_name = &subkeys[index as usize];
    let wide = crate::string_to_wide(key_name);

    if wide.len() > name.len() {
        set_last_error(ctx, ERROR_MORE_DATA);
        return ERROR_MORE_DATA as i32;
    }

    let copy_len = wide.len().min(name.len());
    name[..copy_len].copy_from_slice(&wide[..copy_len]);
    *name_size = (wide.len().saturating_sub(1)) as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_enum_value_w(
    ctx: &mut CompatContext,
    hkey: u64,
    index: u32,
    name: &mut [u16],
    name_size: &mut u32,
    reg_type: &mut u32,
    data: &mut [u8],
    data_size: &mut u32,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    let values = match ctx.registry.enumerate_values(&path) {
        Some(v) => v,
        None => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            return ERROR_FILE_NOT_FOUND as i32;
        }
    };

    if (index as usize) >= values.len() {
        set_last_error(ctx, ERROR_NO_MORE_ITEMS);
        return ERROR_NO_MORE_ITEMS as i32;
    }

    let (val_name, val) = &values[index as usize];
    let wide_name = crate::string_to_wide(val_name);

    if wide_name.len() > name.len() {
        set_last_error(ctx, ERROR_MORE_DATA);
        return ERROR_MORE_DATA as i32;
    }

    let copy_len = wide_name.len().min(name.len());
    name[..copy_len].copy_from_slice(&wide_name[..copy_len]);
    *name_size = (wide_name.len().saturating_sub(1)) as u32;

    let (vtype, raw) = reg_value_to_bytes(val);
    *reg_type = vtype;

    if (*data_size as usize) < raw.len() {
        *data_size = raw.len() as u32;
        set_last_error(ctx, ERROR_MORE_DATA);
        return ERROR_MORE_DATA as i32;
    }

    let copy_data = raw.len().min(data.len());
    data[..copy_data].copy_from_slice(&raw[..copy_data]);
    *data_size = raw.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn reg_flush_key(ctx: &mut CompatContext, hkey: u64) -> i32 {
    if hkey_to_prefix(hkey).is_some() || ctx.handle_table.get(hkey).is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        ERROR_SUCCESS as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        ERROR_INVALID_HANDLE as i32
    }
}

pub fn reg_notify_change_key_value(
    ctx: &mut CompatContext,
    hkey: u64,
    _watch_subtree: bool,
    _filter: u32,
    _event: u64,
    _async_: bool,
) -> i32 {
    if hkey_to_prefix(hkey).is_some() || ctx.handle_table.get(hkey).is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        ERROR_SUCCESS as i32
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        ERROR_INVALID_HANDLE as i32
    }
}

pub fn reg_query_info_key_w(
    ctx: &mut CompatContext,
    hkey: u64,
    _class: Option<&mut [u16]>,
    num_subkeys: &mut u32,
    max_subkey_len: &mut u32,
    num_values: &mut u32,
    max_value_name_len: &mut u32,
    max_value_data_len: &mut u32,
) -> i32 {
    let path = match resolve_key_path(ctx, hkey, "") {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return ERROR_INVALID_HANDLE as i32;
        }
    };

    let prefix = {
        let mut p = path.clone();
        p.push('\\');
        p
    };

    let mut sub_count: u32 = 0;
    let mut max_sub: u32 = 0;
    for key in ctx.registry.enumerate_keys() {
        if let Some(rest) = key.strip_prefix(prefix.as_str()) {
            if !rest.contains('\\') && !rest.is_empty() {
                sub_count += 1;
                max_sub = max_sub.max(rest.len() as u32);
            }
        }
    }
    *num_subkeys = sub_count;
    *max_subkey_len = max_sub;

    match ctx.registry.enumerate_values(&path) {
        Some(values) => {
            *num_values = values.len() as u32;
            let mut max_name: u32 = 0;
            let mut max_data: u32 = 0;
            for (n, v) in &values {
                max_name = max_name.max(n.len() as u32);
                let (_, raw) = reg_value_to_bytes(v);
                max_data = max_data.max(raw.len() as u32);
            }
            *max_value_name_len = max_name;
            *max_value_data_len = max_data;
        }
        None => {
            *num_values = 0;
            *max_value_name_len = 0;
            *max_value_data_len = 0;
        }
    }

    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

// =========================================================================
// Security API
// =========================================================================

pub fn open_process_token(
    ctx: &mut CompatContext,
    _process: u64,
    desired: u32,
    token: &mut u64,
) -> bool {
    let handle = ctx.handle_table.allocate(
        HandleType::Token,
        desired,
        Some(String::from("ProcessToken")),
    );
    *token = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_token_information(
    ctx: &mut CompatContext,
    token: u64,
    info_class: u32,
    info: &mut [u8],
    info_size: &mut u32,
) -> bool {
    if ctx.handle_table.get(token).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    match info_class {
        TOKEN_INFO_ELEVATION => {
            let elevation_data: u32 = 1;
            let bytes = elevation_data.to_le_bytes();
            if info.len() < 4 {
                *info_size = 4;
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return false;
            }
            info[..4].copy_from_slice(&bytes);
            *info_size = 4;
        }
        TOKEN_INFO_USER
        | TOKEN_INFO_GROUPS
        | TOKEN_INFO_PRIVILEGES
        | TOKEN_INFO_OWNER
        | TOKEN_INFO_TYPE => {
            let needed: u32 = 64;
            if (info.len() as u32) < needed {
                *info_size = needed;
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return false;
            }
            for b in info[..needed as usize].iter_mut() {
                *b = 0;
            }
            *info_size = needed;
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return false;
        }
    }

    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn adjust_token_privileges(
    ctx: &mut CompatContext,
    token: u64,
    _disable_all: bool,
    _new_state: &TokenPrivileges,
    _prev_state: Option<&mut TokenPrivileges>,
) -> bool {
    if ctx.handle_table.get(token).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn lookup_account_sid_w(
    ctx: &mut CompatContext,
    _system: Option<&str>,
    _sid: &[u8],
    name: &mut [u16],
    name_size: &mut u32,
    domain: &mut [u16],
    domain_size: &mut u32,
    use_type: &mut u32,
) -> bool {
    let user_name = "user";
    let domain_name = "ATHENAOS";

    let wide_name = crate::string_to_wide(user_name);
    let wide_domain = crate::string_to_wide(domain_name);

    if wide_name.len() > name.len() || wide_domain.len() > domain.len() {
        *name_size = wide_name.len() as u32;
        *domain_size = wide_domain.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }

    let n_copy = wide_name.len().min(name.len());
    name[..n_copy].copy_from_slice(&wide_name[..n_copy]);
    *name_size = (wide_name.len().saturating_sub(1)) as u32;

    let d_copy = wide_domain.len().min(domain.len());
    domain[..d_copy].copy_from_slice(&wide_domain[..d_copy]);
    *domain_size = (wide_domain.len().saturating_sub(1)) as u32;

    *use_type = SID_TYPE_USER;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn lookup_privilege_value_w(
    ctx: &mut CompatContext,
    _system: Option<&str>,
    name: &str,
    luid: &mut u64,
) -> bool {
    let id = match name {
        "SeDebugPrivilege" => 20u64,
        "SeShutdownPrivilege" => 19,
        "SeBackupPrivilege" => 17,
        "SeRestorePrivilege" => 18,
        "SeChangeNotifyPrivilege" => 23,
        "SeSecurityPrivilege" => 8,
        "SeTakeOwnershipPrivilege" => 9,
        "SeLoadDriverPrivilege" => 10,
        "SeImpersonatePrivilege" => 29,
        "SeIncreaseQuotaPrivilege" => 5,
        _ => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            return false;
        }
    };
    *luid = id;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn initialize_security_descriptor(
    ctx: &mut CompatContext,
    sd: &mut [u8],
    revision: u32,
) -> bool {
    if revision != SECURITY_DESCRIPTOR_REVISION {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    if sd.len() < 20 {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    for b in sd.iter_mut() {
        *b = 0;
    }
    sd[0] = revision as u8;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn set_security_descriptor_dacl(
    ctx: &mut CompatContext,
    sd: &mut [u8],
    present: bool,
    _acl: Option<&[u8]>,
    _defaulted: bool,
) -> bool {
    if sd.len() < 20 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    if present {
        sd[2] |= 0x04; // SE_DACL_PRESENT
    } else {
        sd[2] &= !0x04;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_security_info(
    ctx: &mut CompatContext,
    handle: u64,
    _object_type: u32,
    _security_info: u32,
    _owner: Option<&mut u64>,
    _group: Option<&mut u64>,
    _dacl: Option<&mut u64>,
    _sacl: Option<&mut u64>,
    _sd: &mut u64,
) -> u32 {
    if ctx.handle_table.get(handle).is_none() && hkey_to_prefix(handle).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return ERROR_INVALID_HANDLE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn set_security_info(
    ctx: &mut CompatContext,
    handle: u64,
    _object_type: u32,
    _security_info: u32,
    _owner: Option<&[u8]>,
    _group: Option<&[u8]>,
    _dacl: Option<&[u8]>,
    _sacl: Option<&[u8]>,
) -> u32 {
    if ctx.handle_table.get(handle).is_none() && hkey_to_prefix(handle).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return ERROR_INVALID_HANDLE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

// =========================================================================
// Crypto API
// =========================================================================

pub fn crypt_acquire_context_w(
    ctx: &mut CompatContext,
    prov: &mut u64,
    _container: Option<&str>,
    _provider: Option<&str>,
    _prov_type: u32,
    _flags: u32,
) -> bool {
    let handle = ctx.handle_table.allocate(
        HandleType::Token,
        0xFFFFFFFF,
        Some(String::from("CryptProv")),
    );
    *prov = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_release_context(ctx: &mut CompatContext, prov: u64, _flags: u32) -> bool {
    if ctx.handle_table.close(prov) {
        set_last_error(ctx, ERROR_SUCCESS);
        true
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        false
    }
}

pub fn crypt_gen_random(ctx: &mut CompatContext, prov: u64, len: u32, buffer: &mut [u8]) -> bool {
    if ctx.handle_table.get(prov).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    let fill_len = (len as usize).min(buffer.len());
    let mut seed: u32 = prov as u32 ^ len ^ 0xDEADBEEF;
    for b in buffer[..fill_len].iter_mut() {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        *b = (seed >> 16) as u8;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_create_hash(
    ctx: &mut CompatContext,
    prov: u64,
    _algid: u32,
    _key: u64,
    _flags: u32,
    hash: &mut u64,
) -> bool {
    if ctx.handle_table.get(prov).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    let handle = ctx.handle_table.allocate(
        HandleType::Token,
        0xFFFFFFFF,
        Some(String::from("CryptHash")),
    );
    *hash = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_hash_data(ctx: &mut CompatContext, hash: u64, _data: &[u8], _flags: u32) -> bool {
    if ctx.handle_table.get(hash).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_get_hash_param(
    ctx: &mut CompatContext,
    hash: u64,
    param: u32,
    data: &mut [u8],
    data_len: &mut u32,
    _flags: u32,
) -> bool {
    if ctx.handle_table.get(hash).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    match param {
        HP_HASHSIZE => {
            if data.len() < 4 {
                *data_len = 4;
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return false;
            }
            let size: u32 = 32; // SHA-256 default
            data[..4].copy_from_slice(&size.to_le_bytes());
            *data_len = 4;
        }
        HP_HASHVAL => {
            let hash_size: usize = 32;
            if data.len() < hash_size {
                *data_len = hash_size as u32;
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return false;
            }
            for b in data[..hash_size].iter_mut() {
                *b = 0xAA;
            }
            *data_len = hash_size as u32;
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return false;
        }
    }

    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_destroy_hash(ctx: &mut CompatContext, hash: u64) -> bool {
    if ctx.handle_table.close(hash) {
        set_last_error(ctx, ERROR_SUCCESS);
        true
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        false
    }
}

pub fn crypt_derive_key(
    ctx: &mut CompatContext,
    prov: u64,
    _algid: u32,
    _hash: u64,
    _flags: u32,
    key: &mut u64,
) -> bool {
    if ctx.handle_table.get(prov).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    let handle = ctx.handle_table.allocate(
        HandleType::Token,
        0xFFFFFFFF,
        Some(String::from("CryptKey")),
    );
    *key = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_encrypt(
    ctx: &mut CompatContext,
    key: u64,
    _hash: u64,
    _final_: bool,
    _flags: u32,
    data: &mut [u8],
    data_len: &mut u32,
    buf_len: u32,
) -> bool {
    if ctx.handle_table.get(key).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    let current_len = *data_len as usize;
    if current_len > buf_len as usize || current_len > data.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }

    for b in data[..current_len].iter_mut() {
        *b ^= 0x5A;
    }

    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn crypt_decrypt(
    ctx: &mut CompatContext,
    key: u64,
    _hash: u64,
    _final_: bool,
    _flags: u32,
    data: &mut [u8],
    data_len: &mut u32,
) -> bool {
    if ctx.handle_table.get(key).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    let current_len = *data_len as usize;
    if current_len > data.len() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }

    for b in data[..current_len].iter_mut() {
        *b ^= 0x5A;
    }

    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// Event Logging
// =========================================================================

pub fn register_event_source_w(
    ctx: &mut CompatContext,
    _server: Option<&str>,
    source: &str,
) -> u64 {
    let handle =
        ctx.handle_table
            .allocate(HandleType::Event, 0xFFFFFFFF, Some(String::from(source)));
    set_last_error(ctx, ERROR_SUCCESS);
    handle
}

pub fn deregister_event_source(ctx: &mut CompatContext, handle: u64) -> bool {
    if ctx.handle_table.close(handle) {
        set_last_error(ctx, ERROR_SUCCESS);
        true
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        false
    }
}

pub fn report_event_w(
    ctx: &mut CompatContext,
    handle: u64,
    _event_type: u16,
    _category: u16,
    _event_id: u32,
    _sid: Option<&[u8]>,
    _strings: &[&str],
    _data: Option<&[u8]>,
) -> bool {
    if ctx.handle_table.get(handle).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn open_event_log_w(ctx: &mut CompatContext, _server: Option<&str>, source: &str) -> u64 {
    let handle =
        ctx.handle_table
            .allocate(HandleType::Event, 0xFFFFFFFF, Some(String::from(source)));
    set_last_error(ctx, ERROR_SUCCESS);
    handle
}

pub fn close_event_log(ctx: &mut CompatContext, handle: u64) -> bool {
    if ctx.handle_table.close(handle) {
        set_last_error(ctx, ERROR_SUCCESS);
        true
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        false
    }
}

pub fn read_event_log_w(
    ctx: &mut CompatContext,
    handle: u64,
    _flags: u32,
    _offset: u32,
    _buffer: &mut [u8],
    bytes_read: &mut u32,
    min_bytes: &mut u32,
) -> bool {
    if ctx.handle_table.get(handle).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    *bytes_read = 0;
    *min_bytes = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_number_of_event_log_records(
    ctx: &mut CompatContext,
    handle: u64,
    count: &mut u32,
) -> bool {
    if ctx.handle_table.get(handle).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    *count = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// Service Control Manager
// =========================================================================

pub fn open_sc_manager_w(
    ctx: &mut CompatContext,
    _machine: Option<&str>,
    _database: Option<&str>,
    desired: u32,
) -> u64 {
    let handle =
        ctx.handle_table
            .allocate(HandleType::Token, desired, Some(String::from("SCManager")));
    set_last_error(ctx, ERROR_SUCCESS);
    handle
}

pub fn create_service_w(
    ctx: &mut CompatContext,
    scm: u64,
    name: &str,
    _display_name: &str,
    desired: u32,
    _service_type: u32,
    _start_type: u32,
    _error_control: u32,
    _binary_path: &str,
) -> u64 {
    if ctx.handle_table.get(scm).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }

    let handle = ctx
        .handle_table
        .allocate(HandleType::Token, desired, Some(String::from(name)));
    set_last_error(ctx, ERROR_SUCCESS);
    handle
}

pub fn open_service_w(ctx: &mut CompatContext, scm: u64, name: &str, desired: u32) -> u64 {
    if ctx.handle_table.get(scm).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }

    let handle = ctx
        .handle_table
        .allocate(HandleType::Token, desired, Some(String::from(name)));
    set_last_error(ctx, ERROR_SUCCESS);
    handle
}

pub fn close_service_handle(ctx: &mut CompatContext, handle: u64) -> bool {
    if ctx.handle_table.close(handle) {
        set_last_error(ctx, ERROR_SUCCESS);
        true
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        false
    }
}

pub fn start_service_w(ctx: &mut CompatContext, service: u64, _args: &[&str]) -> bool {
    if ctx.handle_table.get(service).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn control_service(
    ctx: &mut CompatContext,
    service: u64,
    control: u32,
    status: &mut ServiceStatus,
) -> bool {
    if ctx.handle_table.get(service).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    match control {
        SERVICE_CONTROL_STOP => {
            *status = ServiceStatus::stopped();
            status.current_state = SERVICE_STOP_PENDING;
        }
        SERVICE_CONTROL_PAUSE => {
            *status = ServiceStatus::running();
            status.current_state = SERVICE_PAUSE_PENDING;
        }
        SERVICE_CONTROL_CONTINUE => {
            *status = ServiceStatus::running();
            status.current_state = SERVICE_CONTINUE_PENDING;
        }
        SERVICE_CONTROL_INTERROGATE => {
            *status = ServiceStatus::running();
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return false;
        }
    }

    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn query_service_status(
    ctx: &mut CompatContext,
    service: u64,
    status: &mut ServiceStatus,
) -> bool {
    if ctx.handle_table.get(service).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }

    *status = ServiceStatus::running();
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn delete_service(ctx: &mut CompatContext, service: u64) -> bool {
    if ctx.handle_table.get(service).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn change_service_config_w(
    ctx: &mut CompatContext,
    service: u64,
    _service_type: u32,
    _start_type: u32,
    _error_control: u32,
    _binary_path: Option<&str>,
    _display_name: Option<&str>,
) -> bool {
    if ctx.handle_table.get(service).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// ANSI string helper
// =========================================================================

fn cstr_to_str(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let mut s = String::new();
    for &b in &raw[..end] {
        s.push(b as char);
    }
    s
}

fn string_to_ansi(s: &str, buf: &mut [u8]) -> usize {
    let bytes = s.as_bytes();
    let copy = core::cmp::min(bytes.len(), buf.len().saturating_sub(1));
    buf[..copy].copy_from_slice(&bytes[..copy]);
    if copy < buf.len() {
        buf[copy] = 0;
    }
    copy
}

// =========================================================================
// Registry API — ANSI variants
// =========================================================================

pub fn reg_open_key_ex_a(
    ctx: &mut CompatContext,
    hkey: u64,
    sub_key: &[u8],
    options: u32,
    desired: u32,
    result: &mut u64,
) -> i32 {
    let key_str = cstr_to_str(sub_key);
    reg_open_key_ex_w(ctx, hkey, &key_str, options, desired, result)
}

pub fn reg_create_key_ex_a(
    ctx: &mut CompatContext,
    hkey: u64,
    sub_key: &[u8],
    reserved: u32,
    class: Option<&[u8]>,
    options: u32,
    desired: u32,
    security: u64,
    result: &mut u64,
    disposition: &mut u32,
) -> i32 {
    let key_str = cstr_to_str(sub_key);
    let class_str = class.map(|c| cstr_to_str(c));
    reg_create_key_ex_w(
        ctx,
        hkey,
        &key_str,
        reserved,
        class_str.as_deref(),
        options,
        desired,
        security,
        result,
        disposition,
    )
}

pub fn reg_delete_key_a(ctx: &mut CompatContext, hkey: u64, sub_key: &[u8]) -> i32 {
    let key_str = cstr_to_str(sub_key);
    reg_delete_key_w(ctx, hkey, &key_str)
}

pub fn reg_delete_value_a(ctx: &mut CompatContext, hkey: u64, value_name: &[u8]) -> i32 {
    let name = cstr_to_str(value_name);
    reg_delete_value_w(ctx, hkey, &name)
}

pub fn reg_set_value_ex_a(
    ctx: &mut CompatContext,
    hkey: u64,
    value_name: &[u8],
    reserved: u32,
    reg_type: u32,
    data: &[u8],
) -> i32 {
    let name = cstr_to_str(value_name);
    reg_set_value_ex_w(ctx, hkey, &name, reserved, reg_type, data)
}

pub fn reg_query_value_ex_a(
    ctx: &mut CompatContext,
    hkey: u64,
    value_name: &[u8],
    reserved: u64,
    reg_type: &mut u32,
    data: &mut [u8],
    data_size: &mut u32,
) -> i32 {
    let name = cstr_to_str(value_name);
    reg_query_value_ex_w(ctx, hkey, &name, reserved, reg_type, data, data_size)
}

pub fn reg_enum_key_ex_a(
    ctx: &mut CompatContext,
    hkey: u64,
    index: u32,
    name: &mut [u8],
    name_size: &mut u32,
) -> i32 {
    let mut wide_buf = alloc::vec![0u16; name.len()];
    let result = reg_enum_key_ex_w(ctx, hkey, index, &mut wide_buf, name_size, None, None);
    if result == ERROR_SUCCESS as i32 {
        let s = crate::wide_to_string(&wide_buf);
        let written = string_to_ansi(&s, name);
        *name_size = written as u32;
    }
    result
}

pub fn reg_enum_value_a(
    ctx: &mut CompatContext,
    hkey: u64,
    index: u32,
    name: &mut [u8],
    name_size: &mut u32,
    reg_type: &mut u32,
    data: &mut [u8],
    data_size: &mut u32,
) -> i32 {
    let mut wide_name = alloc::vec![0u16; name.len()];
    let result = reg_enum_value_w(
        ctx,
        hkey,
        index,
        &mut wide_name,
        name_size,
        reg_type,
        data,
        data_size,
    );
    if result == ERROR_SUCCESS as i32 {
        let s = crate::wide_to_string(&wide_name);
        let written = string_to_ansi(&s, name);
        *name_size = written as u32;
    }
    result
}

// =========================================================================
// Security API — ANSI variants
// =========================================================================

pub fn lookup_account_sid_a(
    ctx: &mut CompatContext,
    system: Option<&[u8]>,
    sid: &[u8],
    name: &mut [u8],
    name_size: &mut u32,
    domain: &mut [u8],
    domain_size: &mut u32,
    use_type: &mut u32,
) -> bool {
    let _ = system;
    let user_name = "user";
    let domain_name = "ATHENAOS";

    let n_needed = user_name.len() + 1;
    let d_needed = domain_name.len() + 1;

    if name.len() < n_needed || domain.len() < d_needed {
        *name_size = n_needed as u32;
        *domain_size = d_needed as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }

    let _ = sid;
    let n_written = string_to_ansi(user_name, name);
    *name_size = n_written as u32;

    let d_written = string_to_ansi(domain_name, domain);
    *domain_size = d_written as u32;

    *use_type = SID_TYPE_USER;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn lookup_privilege_value_a(
    ctx: &mut CompatContext,
    system: Option<&[u8]>,
    name: &[u8],
    luid: &mut u64,
) -> bool {
    let _ = system;
    let name_str = cstr_to_str(name);
    lookup_privilege_value_w(ctx, None, &name_str, luid)
}

// =========================================================================
// Crypto API — ANSI variants
// =========================================================================

pub fn crypt_acquire_context_a(
    ctx: &mut CompatContext,
    prov: &mut u64,
    container: Option<&[u8]>,
    provider: Option<&[u8]>,
    prov_type: u32,
    flags: u32,
) -> bool {
    let _ = container;
    let _ = provider;
    crypt_acquire_context_w(ctx, prov, None, None, prov_type, flags)
}

// =========================================================================
// Service Control Manager — ANSI variants
// =========================================================================

pub fn open_sc_manager_a(
    ctx: &mut CompatContext,
    machine: Option<&[u8]>,
    database: Option<&[u8]>,
    desired: u32,
) -> u64 {
    let _ = machine;
    let _ = database;
    open_sc_manager_w(ctx, None, None, desired)
}

pub fn open_service_a(ctx: &mut CompatContext, scm: u64, name: &[u8], desired: u32) -> u64 {
    let name_str = cstr_to_str(name);
    open_service_w(ctx, scm, &name_str, desired)
}

pub fn start_service_a(ctx: &mut CompatContext, service: u64, _num_args: u32) -> bool {
    let empty: &[&str] = &[];
    start_service_w(ctx, service, empty)
}

pub fn create_service_a(
    ctx: &mut CompatContext,
    scm: u64,
    name: &[u8],
    display_name: &[u8],
    desired: u32,
    service_type: u32,
    start_type: u32,
    error_control: u32,
    binary_path: &[u8],
) -> u64 {
    let name_str = cstr_to_str(name);
    let display_str = cstr_to_str(display_name);
    let path_str = cstr_to_str(binary_path);
    create_service_w(
        ctx,
        scm,
        &name_str,
        &display_str,
        desired,
        service_type,
        start_type,
        error_control,
        &path_str,
    )
}

// =========================================================================
// Event Logging — ANSI variants
// =========================================================================

pub fn register_event_source_a(
    ctx: &mut CompatContext,
    server: Option<&[u8]>,
    source: &[u8],
) -> u64 {
    let _ = server;
    let source_str = cstr_to_str(source);
    register_event_source_w(ctx, None, &source_str)
}

pub fn open_event_log_a(ctx: &mut CompatContext, server: Option<&[u8]>, source: &[u8]) -> u64 {
    let _ = server;
    let source_str = cstr_to_str(source);
    open_event_log_w(ctx, None, &source_str)
}

// =========================================================================
// Additional security helpers
// =========================================================================

pub fn is_valid_sid(_sid: &[u8]) -> bool {
    true
}

pub fn get_length_sid(_sid: &[u8]) -> u32 {
    28 // standard SID length
}

pub fn equal_sid(sid1: &[u8], sid2: &[u8]) -> bool {
    if sid1.len() != sid2.len() {
        return false;
    }
    sid1 == sid2
}

pub fn copy_sid(dest: &mut [u8], src: &[u8]) -> bool {
    if dest.len() < src.len() {
        return false;
    }
    dest[..src.len()].copy_from_slice(src);
    true
}

pub fn convert_sid_to_string_sid_a(
    ctx: &mut CompatContext,
    _sid: &[u8],
    string_sid: &mut [u8],
    string_sid_len: &mut u32,
) -> bool {
    let sid_str = "S-1-5-21-0-0-0-1000";
    let needed = sid_str.len() + 1;
    if string_sid.len() < needed {
        *string_sid_len = needed as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    let written = string_to_ansi(sid_str, string_sid);
    *string_sid_len = written as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn convert_string_sid_to_sid_a(
    ctx: &mut CompatContext,
    _string_sid: &[u8],
    sid: &mut [u8],
    sid_len: &mut u32,
) -> bool {
    let fake_sid: [u8; 28] = [
        1, 5, 0, 0, 0, 0, 0, 5, 21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xE8, 0x03, 0x00,
        0x00,
    ];
    if sid.len() < fake_sid.len() {
        *sid_len = fake_sid.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    sid[..fake_sid.len()].copy_from_slice(&fake_sid);
    *sid_len = fake_sid.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn impersonate_logged_on_user(ctx: &mut CompatContext, token: u64) -> bool {
    if ctx.handle_table.get(token).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn revert_to_self(ctx: &mut CompatContext) -> bool {
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn open_thread_token(
    ctx: &mut CompatContext,
    _thread: u64,
    desired: u32,
    _open_as_self: bool,
    token: &mut u64,
) -> bool {
    let handle = ctx.handle_table.allocate(
        HandleType::Token,
        desired,
        Some(String::from("ThreadToken")),
    );
    *token = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn duplicate_token_ex(
    ctx: &mut CompatContext,
    existing: u64,
    desired: u32,
    _impersonation_level: u32,
    _token_type: u32,
    new_token: &mut u64,
) -> bool {
    if ctx.handle_table.get(existing).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return false;
    }
    let handle =
        ctx.handle_table
            .allocate(HandleType::Token, desired, Some(String::from("DupToken")));
    *new_token = handle;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn set_thread_token(ctx: &mut CompatContext, _thread: Option<u64>, token: Option<u64>) -> bool {
    if let Some(t) = token {
        if ctx.handle_table.get(t).is_none() {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return false;
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_user_name_a(ctx: &mut CompatContext, buffer: &mut [u8], size: &mut u32) -> bool {
    let user = "user";
    let needed = user.len() + 1;
    if (*size as usize) < needed {
        *size = needed as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    let written = string_to_ansi(user, buffer);
    *size = written as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_user_name_w_adv(ctx: &mut CompatContext, buffer: &mut [u16], size: &mut u32) -> bool {
    let user = "user";
    let wide: Vec<u16> = user.encode_utf16().chain(core::iter::once(0)).collect();
    if (*size as usize) < wide.len() {
        *size = wide.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    let copy = wide.len().min(buffer.len());
    buffer[..copy].copy_from_slice(&wide[..copy]);
    *size = (wide.len().saturating_sub(1)) as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}
