//! userenv.dll — User Environment API: user profiles, environment blocks,
//! Group Policy, application containers, special folder mapping, token/SID
//! utilities, and security descriptor functions for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::{
    CompatContext, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_SUCCESS,
};

// =========================================================================
// HResult values
// =========================================================================

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259;
pub const E_INVALIDARG: i32 = -2147024809;
pub const E_ACCESSDENIED: i32 = -2147024891;

// =========================================================================
// Profile type constants
// =========================================================================

pub const PT_TEMPORARY: u32 = 0x00000001;
pub const PT_ROAMING: u32 = 0x00000002;
pub const PT_MANDATORY: u32 = 0x00000004;
pub const PT_ROAMING_PREEXISTING: u32 = 0x00000008;

// =========================================================================
// Group Policy flags
// =========================================================================

pub const RP_FORCE: u32 = 0x00000001;
pub const RP_SYNC: u32 = 0x00000002;

pub const GP_MACHINE: u32 = 0x00000001;
pub const GP_USER: u32 = 0x00000002;

// =========================================================================
// Token information classes
// =========================================================================

pub const TOKEN_USER_INFO: u32 = 1;
pub const TOKEN_GROUPS_INFO: u32 = 2;
pub const TOKEN_PRIVILEGES_INFO: u32 = 3;
pub const TOKEN_OWNER_INFO: u32 = 4;
pub const TOKEN_PRIMARY_GROUP_INFO: u32 = 5;
pub const TOKEN_DEFAULT_DACL_INFO: u32 = 6;
pub const TOKEN_SOURCE_INFO: u32 = 7;
pub const TOKEN_TYPE_INFO: u32 = 8;
pub const TOKEN_IMPERSONATION_LEVEL_INFO: u32 = 9;
pub const TOKEN_STATISTICS_INFO: u32 = 10;
pub const TOKEN_SESSION_ID_INFO: u32 = 12;
pub const TOKEN_ELEVATION_INFO: u32 = 20;
pub const TOKEN_ELEVATION_TYPE_INFO: u32 = 18;
pub const TOKEN_LINKED_TOKEN_INFO: u32 = 19;
pub const TOKEN_INTEGRITY_LEVEL_INFO: u32 = 25;

// =========================================================================
// Security information flags
// =========================================================================

pub const OWNER_SECURITY_INFORMATION: u32 = 0x00000001;
pub const GROUP_SECURITY_INFORMATION: u32 = 0x00000002;
pub const DACL_SECURITY_INFORMATION: u32 = 0x00000004;
pub const SACL_SECURITY_INFORMATION: u32 = 0x00000008;
pub const LABEL_SECURITY_INFORMATION: u32 = 0x00000010;

// =========================================================================
// SE_OBJECT_TYPE
// =========================================================================

pub const SE_FILE_OBJECT: u32 = 1;
pub const SE_SERVICE: u32 = 2;
pub const SE_PRINTER: u32 = 3;
pub const SE_REGISTRY_KEY: u32 = 4;
pub const SE_LMSHARE: u32 = 5;
pub const SE_KERNEL_OBJECT: u32 = 6;
pub const SE_WINDOW_OBJECT: u32 = 7;

// =========================================================================
// ACCESS_MODE
// =========================================================================

pub const NOT_USED_ACCESS: u32 = 0;
pub const GRANT_ACCESS: u32 = 1;
pub const SET_ACCESS: u32 = 2;
pub const DENY_ACCESS: u32 = 3;
pub const REVOKE_ACCESS: u32 = 4;

// =========================================================================
// SDDL revision
// =========================================================================

pub const SDDL_REVISION_1: u32 = 1;

// =========================================================================
// CSIDL / KNOWNFOLDERID mapping
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialFolder {
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    AppDataLocal,
    AppDataRoaming,
    AppDataLocalLow,
    ProgramFiles,
    ProgramFilesX86,
    Windows,
    System32,
    Fonts,
    CommonAppData,
    CommonPrograms,
    CommonStartMenu,
    CommonStartup,
    CommonTemplates,
    UserProfile,
    Public,
}

pub fn special_folder_path(folder: SpecialFolder) -> &'static str {
    match folder {
        SpecialFolder::Desktop => "C:\\Users\\user\\Desktop",
        SpecialFolder::Documents => "C:\\Users\\user\\Documents",
        SpecialFolder::Downloads => "C:\\Users\\user\\Downloads",
        SpecialFolder::Music => "C:\\Users\\user\\Music",
        SpecialFolder::Pictures => "C:\\Users\\user\\Pictures",
        SpecialFolder::Videos => "C:\\Users\\user\\Videos",
        SpecialFolder::AppDataLocal => "C:\\Users\\user\\AppData\\Local",
        SpecialFolder::AppDataRoaming => "C:\\Users\\user\\AppData\\Roaming",
        SpecialFolder::AppDataLocalLow => "C:\\Users\\user\\AppData\\LocalLow",
        SpecialFolder::ProgramFiles => "C:\\Program Files",
        SpecialFolder::ProgramFilesX86 => "C:\\Program Files (x86)",
        SpecialFolder::Windows => "C:\\Windows",
        SpecialFolder::System32 => "C:\\Windows\\System32",
        SpecialFolder::Fonts => "C:\\Windows\\Fonts",
        SpecialFolder::CommonAppData => "C:\\ProgramData",
        SpecialFolder::CommonPrograms => {
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs"
        }
        SpecialFolder::CommonStartMenu => "C:\\ProgramData\\Microsoft\\Windows\\Start Menu",
        SpecialFolder::CommonStartup => {
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Startup"
        }
        SpecialFolder::CommonTemplates => "C:\\ProgramData\\Microsoft\\Windows\\Templates",
        SpecialFolder::UserProfile => "C:\\Users\\user",
        SpecialFolder::Public => "C:\\Users\\Public",
    }
}

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

const DEFAULT_USER_SID: &str = "S-1-5-21-1234567890-1234567890-1234567890-1001";
const SYSTEM_SID: &str = "S-1-5-18";
const ADMIN_GROUP_SID: &str = "S-1-5-32-544";
const USERS_GROUP_SID: &str = "S-1-5-32-545";

// =========================================================================
// User Profiles
// =========================================================================

#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub size: u32,
    pub flags: u32,
    pub user_name: String,
    pub profile_path: Option<String>,
    pub default_path: Option<String>,
    pub server_name: Option<String>,
    pub policy_path: Option<String>,
    pub profile_handle: u64,
}

static NEXT_PROFILE_HANDLE: AtomicU64 = AtomicU64::new(0xA0000);

pub fn load_user_profile_w(
    ctx: &mut CompatContext,
    _token: u64,
    profile: &mut ProfileInfo,
) -> bool {
    if profile.user_name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    profile.profile_handle = NEXT_PROFILE_HANDLE.fetch_add(1, Ordering::SeqCst);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn unload_user_profile(ctx: &mut CompatContext, _token: u64, _profile_handle: u64) -> bool {
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_user_profile_directory_w(
    ctx: &mut CompatContext,
    _token: u64,
    buf: &mut String,
) -> bool {
    buf.clear();
    buf.push_str("C:\\Users\\user");
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_default_user_profile_directory_w(ctx: &mut CompatContext, buf: &mut String) -> bool {
    buf.clear();
    buf.push_str("C:\\Users\\Default");
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_all_users_profile_directory_w(ctx: &mut CompatContext, buf: &mut String) -> bool {
    buf.clear();
    buf.push_str("C:\\ProgramData");
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_profiles_directory_w(ctx: &mut CompatContext, buf: &mut String) -> bool {
    buf.clear();
    buf.push_str("C:\\Users");
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_profile_type(ctx: &mut CompatContext, profile_type: &mut u32) -> bool {
    *profile_type = 0; // Local profile
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn delete_profile_w(
    ctx: &mut CompatContext,
    _sid: &str,
    _profile_path: Option<&str>,
    _computer_name: Option<&str>,
) -> bool {
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn create_profile(
    ctx: &mut CompatContext,
    _sid: &str,
    user_name: &str,
    profile_path: &mut String,
) -> i32 {
    if user_name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return E_INVALIDARG;
    }
    profile_path.clear();
    profile_path.push_str("C:\\Users\\");
    profile_path.push_str(user_name);
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

// =========================================================================
// Environment
// =========================================================================

pub fn create_environment_block(
    ctx: &mut CompatContext,
    _token: u64,
    _inherit: bool,
) -> Vec<(String, String)> {
    set_last_error(ctx, ERROR_SUCCESS);
    let mut env = Vec::new();
    for (k, v) in &ctx.environment {
        env.push((k.clone(), v.clone()));
    }
    if env.is_empty() {
        env.push((String::from("OS"), String::from("Windows_NT")));
        env.push((String::from("SystemRoot"), String::from("C:\\Windows")));
        env.push((String::from("USERPROFILE"), String::from("C:\\Users\\user")));
    }
    env
}

pub fn destroy_environment_block(_block: Vec<(String, String)>) -> bool {
    true
}

pub fn expand_environment_strings_for_user_w(
    ctx: &mut CompatContext,
    _token: u64,
    src: &str,
    dest: &mut String,
) -> bool {
    dest.clear();
    let mut result = String::from(src);
    for (key, value) in &ctx.environment {
        let pattern = alloc::format!("%{}%", key);
        while result.contains(&pattern) {
            result = result.replacen(&pattern, value, 1);
        }
    }
    dest.push_str(&result);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_user_environment_variable(ctx: &mut CompatContext, name: &str) -> Option<String> {
    let val = ctx.environment.get(name).cloned();
    if val.is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        val
    } else {
        set_last_error(ctx, 203); // ERROR_ENVVAR_NOT_FOUND
        None
    }
}

// =========================================================================
// Group Policy
// =========================================================================

static NEXT_GP_NOTIFY: AtomicU64 = AtomicU64::new(0xC0000);

pub fn register_gp_notification(machine_policy: bool) -> u64 {
    let _ = machine_policy;
    NEXT_GP_NOTIFY.fetch_add(1, Ordering::SeqCst)
}

pub fn unregister_gp_notification(_handle: u64) -> bool {
    true
}

static NEXT_CRITICAL_SECTION: AtomicU64 = AtomicU64::new(0xD0000);

pub fn enter_critical_policy_section(machine_policy: bool) -> u64 {
    let _ = machine_policy;
    NEXT_CRITICAL_SECTION.fetch_add(1, Ordering::SeqCst)
}

pub fn leave_critical_policy_section(_handle: u64) -> bool {
    true
}

pub fn refresh_policy(machine_policy: bool) -> bool {
    let _ = machine_policy;
    true
}

pub fn refresh_policy_ex(flags: u32) -> bool {
    let _ = flags;
    true
}

#[derive(Debug, Clone)]
pub struct GpoEntry {
    pub display_name: String,
    pub path: String,
    pub link: String,
    pub options: u32,
}

pub fn get_applied_gpo_list_w(
    _machine: bool,
    _sid: Option<&str>,
    _extension_guid: &[u8; 16],
) -> Vec<GpoEntry> {
    Vec::new()
}

pub fn free_gpo_list_w(_list: Vec<GpoEntry>) {
    // no-op, Rust manages memory
}

// =========================================================================
// Application Directories
// =========================================================================

pub fn get_app_container_folder_path(ctx: &mut CompatContext, sid: &str, path: &mut String) -> i32 {
    if sid.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return E_INVALIDARG;
    }
    path.clear();
    path.push_str("C:\\Users\\user\\AppData\\Local\\Packages\\");
    path.push_str(sid);
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

pub fn derive_app_container_sid_from_app_container_name(_name: &str, sid: &mut String) -> i32 {
    sid.clear();
    sid.push_str(
        "S-1-15-2-1234567890-1234567890-1234567890-1234567890-1234567890-1234567890-1234567890",
    );
    S_OK
}

// =========================================================================
// Token/SID Utilities
// =========================================================================

pub fn convert_sid_to_string_sid_w(
    ctx: &mut CompatContext,
    _sid: &[u8],
    string_sid: &mut String,
) -> bool {
    string_sid.clear();
    string_sid.push_str(DEFAULT_USER_SID);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn convert_string_sid_to_sid_w(
    ctx: &mut CompatContext,
    string_sid: &str,
    sid: &mut Vec<u8>,
) -> bool {
    if string_sid.is_empty() || !string_sid.starts_with("S-") {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    sid.clear();
    sid.extend_from_slice(&[1, 5, 0, 0, 0, 0, 0, 5]); // minimal SID header
    let parts: Vec<&str> = string_sid.split('-').collect();
    for part in parts.iter().skip(3) {
        if let Ok(v) = part.parse::<u32>() {
            sid.extend_from_slice(&v.to_le_bytes());
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub name: String,
    pub domain: String,
    pub sid_type: u32, // SidTypeUser=1, SidTypeGroup=2, etc.
}

pub fn lookup_account_sid_w(
    ctx: &mut CompatContext,
    _system: Option<&str>,
    sid: &str,
) -> Option<AccountInfo> {
    if sid.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return None;
    }
    let info = if sid == SYSTEM_SID {
        AccountInfo {
            name: String::from("SYSTEM"),
            domain: String::from("NT AUTHORITY"),
            sid_type: 5, // SidTypeWellKnownGroup
        }
    } else if sid == ADMIN_GROUP_SID {
        AccountInfo {
            name: String::from("Administrators"),
            domain: String::from("BUILTIN"),
            sid_type: 2, // SidTypeGroup
        }
    } else if sid == USERS_GROUP_SID {
        AccountInfo {
            name: String::from("Users"),
            domain: String::from("BUILTIN"),
            sid_type: 2,
        }
    } else {
        AccountInfo {
            name: String::from("user"),
            domain: String::from("RAEENOS"),
            sid_type: 1, // SidTypeUser
        }
    };
    set_last_error(ctx, ERROR_SUCCESS);
    Some(info)
}

pub fn lookup_account_name_w(
    ctx: &mut CompatContext,
    _system: Option<&str>,
    name: &str,
    sid: &mut String,
) -> bool {
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    sid.clear();
    let lower = name.to_ascii_lowercase();
    if lower == "system" {
        sid.push_str(SYSTEM_SID);
    } else if lower == "administrators" {
        sid.push_str(ADMIN_GROUP_SID);
    } else if lower == "users" {
        sid.push_str(USERS_GROUP_SID);
    } else {
        sid.push_str(DEFAULT_USER_SID);
    }
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// Token information
// =========================================================================

#[derive(Debug, Clone)]
pub enum TokenInfo {
    User {
        sid: String,
    },
    Groups {
        sids: Vec<String>,
    },
    Privileges {
        names: Vec<String>,
    },
    Owner {
        sid: String,
    },
    PrimaryGroup {
        sid: String,
    },
    DefaultDacl {
        acl_present: bool,
    },
    Source {
        name: String,
        id: u64,
    },
    Type {
        is_primary: bool,
    },
    ImpersonationLevel {
        level: u32,
    },
    Statistics {
        token_id: u64,
        auth_id: u64,
        token_type: u32,
    },
    SessionId {
        session: u32,
    },
    Elevation {
        is_elevated: bool,
    },
    ElevationType {
        elevation_type: u32,
    },
    LinkedToken {
        linked_token: u64,
    },
    IntegrityLevel {
        level: u32,
    },
}

pub fn get_token_information(
    ctx: &mut CompatContext,
    _token: u64,
    info_class: u32,
) -> Option<TokenInfo> {
    let info = match info_class {
        TOKEN_USER_INFO => TokenInfo::User {
            sid: String::from(DEFAULT_USER_SID),
        },
        TOKEN_GROUPS_INFO => TokenInfo::Groups {
            sids: alloc::vec![String::from(USERS_GROUP_SID), String::from(ADMIN_GROUP_SID),],
        },
        TOKEN_PRIVILEGES_INFO => TokenInfo::Privileges {
            names: alloc::vec![
                String::from("SeChangeNotifyPrivilege"),
                String::from("SeShutdownPrivilege"),
                String::from("SeUndockPrivilege"),
                String::from("SeIncreaseWorkingSetPrivilege"),
                String::from("SeTimeZonePrivilege"),
            ],
        },
        TOKEN_OWNER_INFO => TokenInfo::Owner {
            sid: String::from(DEFAULT_USER_SID),
        },
        TOKEN_PRIMARY_GROUP_INFO => TokenInfo::PrimaryGroup {
            sid: String::from(USERS_GROUP_SID),
        },
        TOKEN_DEFAULT_DACL_INFO => TokenInfo::DefaultDacl { acl_present: true },
        TOKEN_SOURCE_INFO => TokenInfo::Source {
            name: String::from("User32"),
            id: 0,
        },
        TOKEN_TYPE_INFO => TokenInfo::Type { is_primary: true },
        TOKEN_IMPERSONATION_LEVEL_INFO => TokenInfo::ImpersonationLevel {
            level: 0, // SecurityAnonymous
        },
        TOKEN_STATISTICS_INFO => TokenInfo::Statistics {
            token_id: 0x10000,
            auth_id: 0x3E7,
            token_type: 1, // TokenPrimary
        },
        TOKEN_SESSION_ID_INFO => TokenInfo::SessionId { session: 1 },
        TOKEN_ELEVATION_INFO => TokenInfo::Elevation { is_elevated: false },
        TOKEN_ELEVATION_TYPE_INFO => TokenInfo::ElevationType {
            elevation_type: 3, // TokenElevationTypeLimited
        },
        TOKEN_LINKED_TOKEN_INFO => TokenInfo::LinkedToken { linked_token: 0 },
        TOKEN_INTEGRITY_LEVEL_INFO => TokenInfo::IntegrityLevel {
            level: 0x2000, // SECURITY_MANDATORY_MEDIUM_RID
        },
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return None;
        }
    };
    set_last_error(ctx, ERROR_SUCCESS);
    Some(info)
}

// =========================================================================
// Security Descriptors
// =========================================================================

#[derive(Debug, Clone)]
pub struct ExplicitAccess {
    pub permissions: u32,
    pub access_mode: u32,
    pub inheritance: u32,
    pub trustee_name: String,
    pub trustee_type: u32,
}

pub fn get_security_info(
    ctx: &mut CompatContext,
    _handle: u64,
    _object_type: u32,
    _security_info: u32,
    owner_sid: &mut Option<String>,
    group_sid: &mut Option<String>,
) -> u32 {
    *owner_sid = Some(String::from(DEFAULT_USER_SID));
    *group_sid = Some(String::from(USERS_GROUP_SID));
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn set_security_info(
    ctx: &mut CompatContext,
    _handle: u64,
    _object_type: u32,
    _security_info: u32,
    _owner_sid: Option<&str>,
    _group_sid: Option<&str>,
) -> u32 {
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn get_named_security_info_w(
    ctx: &mut CompatContext,
    _object_name: &str,
    _object_type: u32,
    _security_info: u32,
    owner_sid: &mut Option<String>,
    group_sid: &mut Option<String>,
) -> u32 {
    *owner_sid = Some(String::from(DEFAULT_USER_SID));
    *group_sid = Some(String::from(USERS_GROUP_SID));
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn set_named_security_info_w(
    ctx: &mut CompatContext,
    _object_name: &str,
    _object_type: u32,
    _security_info: u32,
    _owner_sid: Option<&str>,
    _group_sid: Option<&str>,
) -> u32 {
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn build_explicit_access_with_name_w(
    trustee_name: &str,
    permissions: u32,
    access_mode: u32,
    inheritance: u32,
) -> ExplicitAccess {
    ExplicitAccess {
        permissions,
        access_mode,
        inheritance,
        trustee_name: String::from(trustee_name),
        trustee_type: 0, // TRUSTEE_IS_UNKNOWN
    }
}

pub fn set_entries_in_acl_w(_entries: &[ExplicitAccess], _old_acl: u64, new_acl: &mut u64) -> u32 {
    *new_acl = 0xAC100001;
    ERROR_SUCCESS
}

pub fn convert_security_descriptor_to_string_security_descriptor_w(
    ctx: &mut CompatContext,
    _sd: u64,
    _revision: u32,
    _security_info: u32,
    string_sd: &mut String,
) -> bool {
    string_sd.clear();
    string_sd.push_str("D:(A;;GA;;;BA)(A;;GA;;;SY)(A;;GXGR;;;BU)");
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn convert_string_security_descriptor_to_security_descriptor_w(
    ctx: &mut CompatContext,
    string_sd: &str,
    _revision: u32,
    sd: &mut u64,
) -> bool {
    if string_sd.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }
    *sd = 0x5D000001;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// Global USER_ENV runtime
// =========================================================================

static USERENV_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct UserEnvRuntime {
    pub loaded_profiles: BTreeMap<u64, ProfileInfo>,
    pub gp_notifications: Vec<u64>,
    pub special_folders: BTreeMap<String, SpecialFolder>,
}

impl UserEnvRuntime {
    fn new() -> Self {
        let mut folders = BTreeMap::new();
        folders.insert(String::from("Desktop"), SpecialFolder::Desktop);
        folders.insert(String::from("Documents"), SpecialFolder::Documents);
        folders.insert(String::from("Downloads"), SpecialFolder::Downloads);
        folders.insert(String::from("Music"), SpecialFolder::Music);
        folders.insert(String::from("Pictures"), SpecialFolder::Pictures);
        folders.insert(String::from("Videos"), SpecialFolder::Videos);
        folders.insert(String::from("AppData_Local"), SpecialFolder::AppDataLocal);
        folders.insert(
            String::from("AppData_Roaming"),
            SpecialFolder::AppDataRoaming,
        );
        folders.insert(
            String::from("AppData_LocalLow"),
            SpecialFolder::AppDataLocalLow,
        );
        folders.insert(String::from("ProgramFiles"), SpecialFolder::ProgramFiles);
        folders.insert(
            String::from("ProgramFiles_x86"),
            SpecialFolder::ProgramFilesX86,
        );
        folders.insert(String::from("Windows"), SpecialFolder::Windows);
        folders.insert(String::from("System32"), SpecialFolder::System32);
        folders.insert(String::from("Fonts"), SpecialFolder::Fonts);
        folders.insert(String::from("CommonAppData"), SpecialFolder::CommonAppData);
        folders.insert(
            String::from("CommonPrograms"),
            SpecialFolder::CommonPrograms,
        );
        folders.insert(
            String::from("CommonStartMenu"),
            SpecialFolder::CommonStartMenu,
        );
        folders.insert(String::from("CommonStartup"), SpecialFolder::CommonStartup);
        folders.insert(
            String::from("CommonTemplates"),
            SpecialFolder::CommonTemplates,
        );
        folders.insert(String::from("UserProfile"), SpecialFolder::UserProfile);
        folders.insert(String::from("Public"), SpecialFolder::Public);
        Self {
            loaded_profiles: BTreeMap::new(),
            gp_notifications: Vec::new(),
            special_folders: folders,
        }
    }

    pub fn resolve_folder(&self, name: &str) -> Option<&'static str> {
        self.special_folders
            .get(name)
            .map(|f| special_folder_path(*f))
    }
}

static mut USERENV_INNER: Option<UserEnvRuntime> = None;

pub fn init() {
    if USERENV_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            USERENV_INNER = Some(UserEnvRuntime::new());
        }
    }
}

pub fn runtime() -> Option<&'static UserEnvRuntime> {
    if USERENV_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { USERENV_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut UserEnvRuntime> {
    if USERENV_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { USERENV_INNER.as_mut() }
    } else {
        None
    }
}
