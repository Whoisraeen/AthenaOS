//! shell32.dll — Shell operations, file dialogs, shell folders, drag-drop,
//! notify icons, path utilities, and recycle bin APIs for AthBridge.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    translate_win_path, CompatContext, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
    ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_SUCCESS,
};

// =========================================================================
// CSIDL folder constants
// =========================================================================

pub const CSIDL_DESKTOP: i32 = 0x0000;
pub const CSIDL_INTERNET: i32 = 0x0001;
pub const CSIDL_PROGRAMS: i32 = 0x0002;
pub const CSIDL_CONTROLS: i32 = 0x0003;
pub const CSIDL_PRINTERS: i32 = 0x0004;
pub const CSIDL_PERSONAL: i32 = 0x0005;
pub const CSIDL_FAVORITES: i32 = 0x0006;
pub const CSIDL_STARTUP: i32 = 0x0007;
pub const CSIDL_RECENT: i32 = 0x0008;
pub const CSIDL_SENDTO: i32 = 0x0009;
pub const CSIDL_BITBUCKET: i32 = 0x000A;
pub const CSIDL_STARTMENU: i32 = 0x000B;
pub const CSIDL_MYDOCUMENTS: i32 = 0x000C;
pub const CSIDL_MYMUSIC: i32 = 0x000D;
pub const CSIDL_MYVIDEO: i32 = 0x000E;
pub const CSIDL_DESKTOPDIRECTORY: i32 = 0x0010;
pub const CSIDL_DRIVES: i32 = 0x0011;
pub const CSIDL_NETWORK: i32 = 0x0012;
pub const CSIDL_NETHOOD: i32 = 0x0013;
pub const CSIDL_FONTS: i32 = 0x0014;
pub const CSIDL_TEMPLATES: i32 = 0x0015;
pub const CSIDL_COMMON_STARTMENU: i32 = 0x0016;
pub const CSIDL_COMMON_PROGRAMS: i32 = 0x0017;
pub const CSIDL_COMMON_STARTUP: i32 = 0x0018;
pub const CSIDL_COMMON_DESKTOPDIRECTORY: i32 = 0x0019;
pub const CSIDL_APPDATA: i32 = 0x001A;
pub const CSIDL_PRINTHOOD: i32 = 0x001B;
pub const CSIDL_LOCAL_APPDATA: i32 = 0x001C;
pub const CSIDL_COMMON_APPDATA: i32 = 0x0023;
pub const CSIDL_WINDOWS: i32 = 0x0024;
pub const CSIDL_SYSTEM: i32 = 0x0025;
pub const CSIDL_PROGRAM_FILES: i32 = 0x0026;
pub const CSIDL_MYPICTURES: i32 = 0x0027;
pub const CSIDL_PROFILE: i32 = 0x0028;
pub const CSIDL_PROGRAM_FILES_COMMON: i32 = 0x002B;
pub const CSIDL_COMMON_TEMPLATES: i32 = 0x002D;
pub const CSIDL_COMMON_DOCUMENTS: i32 = 0x002E;
pub const CSIDL_COMMON_ADMINTOOLS: i32 = 0x002F;
pub const CSIDL_ADMINTOOLS: i32 = 0x0030;
pub const CSIDL_RESOURCES: i32 = 0x0038;

// =========================================================================
// Known folder GUIDs (as byte arrays for no_std)
// =========================================================================

pub const FOLDERID_DOCUMENTS: [u8; 16] = [
    0xED, 0x4B, 0xAD, 0xFD, 0xE7, 0x9D, 0x45, 0xD6, 0xA1, 0x6E, 0x72, 0x81, 0x0A, 0xAE, 0x89, 0x0E,
];
pub const FOLDERID_DOWNLOADS: [u8; 16] = [
    0x09, 0x4B, 0x9F, 0x37, 0x4C, 0x0A, 0x4D, 0x49, 0x86, 0x55, 0x2B, 0x1B, 0xCA, 0x60, 0x74, 0x81,
];
pub const FOLDERID_DESKTOP: [u8; 16] = [
    0x1D, 0x9F, 0x54, 0xB3, 0xA3, 0x5D, 0x4F, 0x4B, 0x88, 0x0D, 0x15, 0x0E, 0xE3, 0xC4, 0x79, 0x84,
];
pub const FOLDERID_PROFILE: [u8; 16] = [
    0x60, 0x2E, 0xA6, 0x5E, 0xF5, 0x3F, 0x48, 0x26, 0xAA, 0xAC, 0x1B, 0x7C, 0xC7, 0x12, 0x06, 0x3F,
];

// =========================================================================
// Shell file operation constants
// =========================================================================

pub const FO_MOVE: u32 = 0x0001;
pub const FO_COPY: u32 = 0x0002;
pub const FO_DELETE: u32 = 0x0003;
pub const FO_RENAME: u32 = 0x0004;

pub const FOF_MULTIDESTFILES: u16 = 0x0001;
pub const FOF_CONFIRMMOUSE: u16 = 0x0002;
pub const FOF_SILENT: u16 = 0x0004;
pub const FOF_RENAMEONCOLLISION: u16 = 0x0008;
pub const FOF_NOCONFIRMATION: u16 = 0x0010;
pub const FOF_NOERRORUI: u16 = 0x0400;
pub const FOF_NOCOPYSECURITYATTRIBS: u16 = 0x0800;
pub const FOF_ALLOWUNDO: u16 = 0x0040;
pub const FOF_FILESONLY: u16 = 0x0080;
pub const FOF_SIMPLEPROGRESS: u16 = 0x0100;
pub const FOF_NOCONFIRMMKDIR: u16 = 0x0200;

// =========================================================================
// ShellExecute show commands
// =========================================================================

pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNORMAL: i32 = 1;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMAXIMIZED: i32 = 3;
pub const SW_SHOW: i32 = 5;

// =========================================================================
// ShellExecuteEx masks
// =========================================================================

pub const SEE_MASK_DEFAULT: u32 = 0x00000000;
pub const SEE_MASK_CLASSNAME: u32 = 0x00000001;
pub const SEE_MASK_CLASSKEY: u32 = 0x00000003;
pub const SEE_MASK_IDLIST: u32 = 0x00000004;
pub const SEE_MASK_INVOKEIDLIST: u32 = 0x0000000C;
pub const SEE_MASK_NOCLOSEPROCESS: u32 = 0x00000040;
pub const SEE_MASK_FLAG_NO_UI: u32 = 0x00000400;
pub const SEE_MASK_UNICODE: u32 = 0x00004000;
pub const SEE_MASK_NO_CONSOLE: u32 = 0x00008000;
pub const SEE_MASK_NOASYNC: u32 = 0x00000100;

// =========================================================================
// Notify icon constants
// =========================================================================

pub const NIM_ADD: u32 = 0x00000000;
pub const NIM_MODIFY: u32 = 0x00000001;
pub const NIM_DELETE: u32 = 0x00000002;
pub const NIM_SETFOCUS: u32 = 0x00000003;
pub const NIM_SETVERSION: u32 = 0x00000004;

pub const NIF_MESSAGE: u32 = 0x00000001;
pub const NIF_ICON: u32 = 0x00000002;
pub const NIF_TIP: u32 = 0x00000004;
pub const NIF_STATE: u32 = 0x00000008;
pub const NIF_INFO: u32 = 0x00000010;
pub const NIF_GUID: u32 = 0x00000020;
pub const NIF_REALTIME: u32 = 0x00000040;
pub const NIF_SHOWTIP: u32 = 0x00000080;

pub const NIIF_NONE: u32 = 0x00000000;
pub const NIIF_INFO: u32 = 0x00000001;
pub const NIIF_WARNING: u32 = 0x00000002;
pub const NIIF_ERROR: u32 = 0x00000003;

// =========================================================================
// Recycle bin flags
// =========================================================================

pub const SHERB_NOCONFIRMATION: u32 = 0x00000001;
pub const SHERB_NOPROGRESSUI: u32 = 0x00000002;
pub const SHERB_NOSOUND: u32 = 0x00000004;

// =========================================================================
// Open/Save file dialog flags
// =========================================================================

pub const OFN_READONLY: u32 = 0x00000001;
pub const OFN_OVERWRITEPROMPT: u32 = 0x00000002;
pub const OFN_HIDEREADONLY: u32 = 0x00000004;
pub const OFN_NOCHANGEDIR: u32 = 0x00000008;
pub const OFN_ALLOWMULTISELECT: u32 = 0x00000200;
pub const OFN_PATHMUSTEXIST: u32 = 0x00000800;
pub const OFN_FILEMUSTEXIST: u32 = 0x00001000;
pub const OFN_CREATEPROMPT: u32 = 0x00002000;
pub const OFN_NOVALIDATE: u32 = 0x00000100;
pub const OFN_EXPLORER: u32 = 0x00080000;

// =========================================================================
// Browse for folder flags
// =========================================================================

pub const BIF_RETURNONLYFSDIRS: u32 = 0x00000001;
pub const BIF_EDITBOX: u32 = 0x00000010;
pub const BIF_NEWDIALOGSTYLE: u32 = 0x00000040;
pub const BIF_USENEWUI: u32 = 0x00000050;

// =========================================================================
// AssocQueryString types
// =========================================================================

pub const ASSOCSTR_COMMAND: u32 = 1;
pub const ASSOCSTR_EXECUTABLE: u32 = 2;
pub const ASSOCSTR_FRIENDLYDOCNAME: u32 = 3;
pub const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
pub const ASSOCSTR_CONTENTTYPE: u32 = 6;
pub const ASSOCSTR_DEFAULTICON: u32 = 7;

pub const ASSOCF_NONE: u32 = 0x00000000;
pub const ASSOCF_INIT_NOREMAPCLSID: u32 = 0x00000001;
pub const ASSOCF_INIT_BYEXENAME: u32 = 0x00000002;

// =========================================================================
// HResult values used by shell APIs
// =========================================================================

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259; // 0x80004005
pub const E_INVALIDARG: i32 = -2147024809; // 0x80070057
pub const E_OUTOFMEMORY: i32 = -2147024882; // 0x8007000E

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct ShFileOp {
    pub hwnd: u64,
    pub func: u32,
    pub from: String,
    pub to: Option<String>,
    pub flags: u16,
    pub any_aborted: bool,
    pub name_mappings: u64,
}

#[derive(Debug, Clone)]
pub struct ShellExecuteInfo {
    pub size: u32,
    pub mask: u32,
    pub hwnd: u64,
    pub verb: Option<String>,
    pub file: String,
    pub parameters: Option<String>,
    pub directory: Option<String>,
    pub show: i32,
    pub inst_app: u64,
    pub id_list: u64,
    pub class: Option<String>,
    pub hkey_class: u64,
    pub hot_key: u32,
    pub icon: u64,
    pub process: u64,
}

#[derive(Debug, Clone)]
pub struct NotifyIconData {
    pub size: u32,
    pub hwnd: u64,
    pub id: u32,
    pub flags: u32,
    pub callback_message: u32,
    pub icon: u64,
    pub tip: String,
    pub state: u32,
    pub state_mask: u32,
    pub info: String,
    pub timeout_or_version: u32,
    pub info_title: String,
    pub info_flags: u32,
    pub guid: [u8; 16],
    pub balloon_icon: u64,
}

#[derive(Debug, Clone)]
pub struct RecycleBinInfo {
    pub size: u64,
    pub num_items: u64,
}

#[derive(Debug, Clone)]
pub struct OpenFileName {
    pub size: u32,
    pub owner: u64,
    pub filter: String,
    pub custom_filter: Option<String>,
    pub filter_index: u32,
    pub file: String,
    pub max_file: u32,
    pub file_title: String,
    pub initial_dir: Option<String>,
    pub title: Option<String>,
    pub flags: u32,
    pub file_offset: u16,
    pub file_extension: u16,
    pub def_ext: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BrowseInfo {
    pub owner: u64,
    pub root: u64,
    pub display_name: String,
    pub title: String,
    pub flags: u32,
    pub callback: u64,
    pub lparam: u64,
    pub image: i32,
}

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

const MAX_PATH: usize = 260;

fn csidl_to_path(csidl: i32) -> Option<&'static str> {
    match csidl & 0xFF {
        0x0000 => Some("C:\\Users\\user\\Desktop"),
        0x0002 => {
            Some("C:\\Users\\user\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs")
        }
        0x0005 => Some("C:\\Users\\user\\Documents"),
        0x0006 => Some("C:\\Users\\user\\Favorites"),
        0x0007 => Some(
            "C:\\Users\\user\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup",
        ),
        0x0008 => Some("C:\\Users\\user\\AppData\\Roaming\\Microsoft\\Windows\\Recent"),
        0x000B => Some("C:\\Users\\user\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu"),
        0x000D => Some("C:\\Users\\user\\Music"),
        0x000E => Some("C:\\Users\\user\\Videos"),
        0x0010 => Some("C:\\Users\\user\\Desktop"),
        0x0014 => Some("C:\\Windows\\Fonts"),
        0x001A => Some("C:\\Users\\user\\AppData\\Roaming"),
        0x001C => Some("C:\\Users\\user\\AppData\\Local"),
        0x0023 => Some("C:\\ProgramData"),
        0x0024 => Some("C:\\Windows"),
        0x0025 => Some("C:\\Windows\\System32"),
        0x0026 => Some("C:\\Program Files"),
        0x0027 => Some("C:\\Users\\user\\Pictures"),
        0x0028 => Some("C:\\Users\\user"),
        0x002B => Some("C:\\Program Files\\Common Files"),
        _ => None,
    }
}

// =========================================================================
// Shell Folder Operations
// =========================================================================

pub fn sh_get_folder_path_w(
    ctx: &mut CompatContext,
    _hwnd: u64,
    csidl: i32,
    _token: u64,
    _flags: u32,
    path: &mut [u16],
) -> i32 {
    let folder = match csidl_to_path(csidl) {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return E_INVALIDARG;
        }
    };

    let wide = crate::string_to_wide(folder);
    if wide.len() > path.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return E_FAIL;
    }

    let copy_len = wide.len().min(path.len());
    path[..copy_len].copy_from_slice(&wide[..copy_len]);
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

pub fn sh_get_known_folder_path(
    ctx: &mut CompatContext,
    folder_id: &[u8; 16],
    _flags: u32,
    _token: u64,
    path: &mut u64,
) -> i32 {
    let folder_str = if *folder_id == FOLDERID_DOCUMENTS {
        "C:\\Users\\user\\Documents"
    } else if *folder_id == FOLDERID_DOWNLOADS {
        "C:\\Users\\user\\Downloads"
    } else if *folder_id == FOLDERID_DESKTOP {
        "C:\\Users\\user\\Desktop"
    } else if *folder_id == FOLDERID_PROFILE {
        "C:\\Users\\user"
    } else {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return E_INVALIDARG;
    };

    let _ = folder_str;
    *path = 0x80000000; // opaque pointer placeholder
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

pub fn sh_create_directory_ex_w(
    ctx: &mut CompatContext,
    _hwnd: u64,
    path: &str,
    _security: u64,
) -> i32 {
    if path.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return ERROR_INVALID_PARAMETER as i32;
    }
    let _native = translate_win_path(path);
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS as i32
}

pub fn sh_file_operation_w(ctx: &mut CompatContext, op: &ShFileOp) -> i32 {
    match op.func {
        FO_MOVE | FO_COPY | FO_DELETE | FO_RENAME => {
            if op.from.is_empty() {
                set_last_error(ctx, ERROR_INVALID_PARAMETER);
                return ERROR_INVALID_PARAMETER as i32;
            }
            set_last_error(ctx, ERROR_SUCCESS);
            0
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            ERROR_INVALID_PARAMETER as i32
        }
    }
}

// =========================================================================
// Shell Execute
// =========================================================================

pub fn shell_execute_w(
    ctx: &mut CompatContext,
    _hwnd: u64,
    _operation: Option<&str>,
    file: &str,
    _parameters: Option<&str>,
    _directory: Option<&str>,
    _show_cmd: i32,
) -> u64 {
    if file.is_empty() {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return 0;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    42 // value > 32 indicates success per Win32 convention
}

pub fn shell_execute_ex_w(ctx: &mut CompatContext, info: &mut ShellExecuteInfo) -> bool {
    if info.file.is_empty() {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return false;
    }

    if info.mask & SEE_MASK_NOCLOSEPROCESS != 0 {
        info.process = 0xFACE0000;
    }

    info.inst_app = 42;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

// =========================================================================
// Drag and Drop
// =========================================================================

pub fn drag_accept_files(_hwnd: u64, _accept: bool) {
    // Stub — sets/clears WS_EX_ACCEPTFILES on the emulated window.
}

pub fn drag_query_file_w(_drop: u64, index: u32, file: &mut [u16], _size: u32) -> u32 {
    if index == 0xFFFFFFFF {
        return 0;
    }
    if file.is_empty() {
        return 0;
    }
    let placeholder = crate::string_to_wide("C:\\dropped_file.txt");
    let copy_len = placeholder.len().min(file.len());
    file[..copy_len].copy_from_slice(&placeholder[..copy_len]);
    placeholder.len() as u32
}

pub fn drag_query_point(_drop: u64, pt: &mut (i32, i32)) -> bool {
    pt.0 = 0;
    pt.1 = 0;
    true
}

pub fn drag_finish(_drop: u64) {
    // Release drop resources — no-op in emulation.
}

// =========================================================================
// Notify Icon (System Tray)
// =========================================================================

pub fn shell_notify_icon_w(ctx: &mut CompatContext, message: u32, _data: &NotifyIconData) -> bool {
    match message {
        NIM_ADD | NIM_MODIFY | NIM_DELETE | NIM_SETFOCUS | NIM_SETVERSION => {
            set_last_error(ctx, ERROR_SUCCESS);
            true
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            false
        }
    }
}

// =========================================================================
// Path Operations
// =========================================================================

pub fn path_combine_w(dest: &mut [u16], dir: &[u16], file: &[u16]) -> bool {
    let d = crate::wide_to_string(dir);
    let f = crate::wide_to_string(file);

    let combined = if d.is_empty() {
        f
    } else if f.is_empty() {
        d
    } else {
        let mut result = d;
        if !result.ends_with('\\') && !result.ends_with('/') {
            result.push('\\');
        }
        result.push_str(&f);
        result
    };

    let wide = crate::string_to_wide(&combined);
    if wide.len() > dest.len() {
        return false;
    }
    let copy_len = wide.len().min(dest.len());
    dest[..copy_len].copy_from_slice(&wide[..copy_len]);
    true
}

pub fn path_find_file_name_w(path: &str) -> String {
    if let Some(pos) = path.rfind('\\') {
        String::from(&path[pos + 1..])
    } else if let Some(pos) = path.rfind('/') {
        String::from(&path[pos + 1..])
    } else {
        String::from(path)
    }
}

pub fn path_find_extension_w(path: &str) -> String {
    let filename = path_find_file_name_w(path);
    if let Some(pos) = filename.rfind('.') {
        String::from(&filename[pos..])
    } else {
        String::new()
    }
}

pub fn path_remove_file_spec_w(path: &mut String) -> bool {
    if let Some(pos) = path.rfind('\\') {
        path.truncate(pos);
        true
    } else if let Some(pos) = path.rfind('/') {
        path.truncate(pos);
        true
    } else {
        false
    }
}

pub fn path_is_directory_w(_path: &str) -> bool {
    false
}

pub fn path_file_exists_w(_path: &str) -> bool {
    false
}

pub fn path_add_backslash_w(path: &mut String) {
    if !path.ends_with('\\') && !path.ends_with('/') {
        path.push('\\');
    }
}

pub fn path_remove_backslash_w(path: &mut String) {
    if path.len() > 1 && (path.ends_with('\\') || path.ends_with('/')) {
        path.pop();
    }
}

pub fn path_is_relative_w(path: &str) -> bool {
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        return false;
    }
    if path.starts_with('\\') || path.starts_with('/') {
        return false;
    }
    true
}

pub fn path_canonicalize_w(dest: &mut String, path: &str) -> bool {
    let parts: Vec<&str> = path.split(|c| c == '\\' || c == '/').collect();
    let mut stack: Vec<&str> = Vec::new();

    for part in &parts {
        match *part {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => {
                stack.push(other);
            }
        }
    }

    dest.clear();
    if path.starts_with('\\') || path.starts_with('/') {
        dest.push('\\');
    } else if path.len() >= 2 && path.as_bytes()[1] == b':' {
        if !stack.is_empty() {
            dest.push_str(stack[0]);
            dest.push('\\');
            for (i, part) in stack.iter().enumerate().skip(1) {
                if i > 1 {
                    dest.push('\\');
                }
                dest.push_str(part);
            }
            return true;
        }
    }

    for (i, part) in stack.iter().enumerate() {
        if i > 0 {
            dest.push('\\');
        }
        dest.push_str(part);
    }
    true
}

// =========================================================================
// File Type Association
// =========================================================================

pub fn assoc_query_string_w(
    ctx: &mut CompatContext,
    _flags: u32,
    str_type: u32,
    assoc: &str,
    _extra: Option<&str>,
    result: &mut [u16],
    result_size: &mut u32,
) -> i32 {
    let value = match str_type {
        ASSOCSTR_EXECUTABLE => {
            let mut exe = String::from("C:\\Program Files\\");
            exe.push_str(assoc);
            exe.push_str("\\app.exe");
            exe
        }
        ASSOCSTR_FRIENDLYAPPNAME => {
            let mut name = String::from(assoc);
            name.push_str(" Application");
            name
        }
        ASSOCSTR_FRIENDLYDOCNAME => {
            let mut name = String::from(assoc);
            name.push_str(" Document");
            name
        }
        ASSOCSTR_CONTENTTYPE => String::from("application/octet-stream"),
        ASSOCSTR_COMMAND | ASSOCSTR_DEFAULTICON => String::from(""),
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return E_INVALIDARG;
        }
    };

    let wide = crate::string_to_wide(&value);
    if (wide.len() as u32) > *result_size || wide.len() > result.len() {
        *result_size = wide.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return S_FALSE;
    }

    let copy_len = wide.len().min(result.len());
    result[..copy_len].copy_from_slice(&wide[..copy_len]);
    *result_size = wide.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

// =========================================================================
// Recycle Bin
// =========================================================================

pub fn sh_empty_recycle_bin_w(
    ctx: &mut CompatContext,
    _hwnd: u64,
    _root: Option<&str>,
    _flags: u32,
) -> i32 {
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

pub fn sh_query_recycle_bin_w(
    ctx: &mut CompatContext,
    _root: Option<&str>,
    info: &mut RecycleBinInfo,
) -> i32 {
    info.size = 0;
    info.num_items = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

// =========================================================================
// Common Dialogs
// =========================================================================

pub fn get_open_file_name_w(ctx: &mut CompatContext, ofn: &mut OpenFileName) -> bool {
    if ofn.file.is_empty() && ofn.max_file == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }

    ofn.file = String::from("C:\\Users\\user\\Documents\\selected_file.txt");
    ofn.file_title = String::from("selected_file.txt");
    ofn.file_offset = 32;
    ofn.file_extension = 46;
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_save_file_name_w(ctx: &mut CompatContext, ofn: &mut OpenFileName) -> bool {
    if ofn.file.is_empty() && ofn.max_file == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }

    if ofn.file.is_empty() {
        ofn.file = String::from("C:\\Users\\user\\Documents\\new_file.txt");
    }
    ofn.file_title = path_find_file_name_w(&ofn.file);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn sh_browse_for_folder_w(ctx: &mut CompatContext, _bi: &BrowseInfo) -> u64 {
    set_last_error(ctx, ERROR_SUCCESS);
    0x80001000 // opaque PIDL placeholder
}

// =========================================================================
// ANSI variants
// =========================================================================

fn cstr_to_string(ptr: &[u8]) -> String {
    let end = ptr.iter().position(|&b| b == 0).unwrap_or(ptr.len());
    let mut s = String::new();
    for &b in &ptr[..end] {
        s.push(b as char);
    }
    s
}

fn string_to_ansi_buf(s: &str, buf: &mut [u8]) -> usize {
    let bytes = s.as_bytes();
    let copy_len = core::cmp::min(bytes.len(), buf.len().saturating_sub(1));
    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    if copy_len < buf.len() {
        buf[copy_len] = 0;
    }
    copy_len
}

pub fn sh_get_folder_path_a(
    ctx: &mut CompatContext,
    hwnd: u64,
    csidl: i32,
    token: u64,
    flags: u32,
    path: &mut [u8],
) -> i32 {
    let folder = match csidl_to_path(csidl) {
        Some(p) => p,
        None => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return E_INVALIDARG;
        }
    };

    let _ = (hwnd, token, flags);

    if folder.len() + 1 > path.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return E_FAIL;
    }

    string_to_ansi_buf(folder, path);
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

pub fn sh_get_special_folder_path_w(
    ctx: &mut CompatContext,
    _hwnd: u64,
    path: &mut [u16],
    csidl: i32,
    _create: bool,
) -> bool {
    let result = sh_get_folder_path_w(ctx, 0, csidl, 0, 0, path);
    result == S_OK
}

pub fn sh_get_special_folder_path_a(
    ctx: &mut CompatContext,
    _hwnd: u64,
    path: &mut [u8],
    csidl: i32,
    _create: bool,
) -> bool {
    let result = sh_get_folder_path_a(ctx, 0, csidl, 0, 0, path);
    result == S_OK
}

pub fn shell_execute_a(
    ctx: &mut CompatContext,
    hwnd: u64,
    operation: Option<&[u8]>,
    file: &[u8],
    parameters: Option<&[u8]>,
    directory: Option<&[u8]>,
    show_cmd: i32,
) -> u64 {
    let op = operation.map(|o| cstr_to_string(o));
    let f = cstr_to_string(file);
    let params = parameters.map(|p| cstr_to_string(p));
    let dir = directory.map(|d| cstr_to_string(d));

    shell_execute_w(
        ctx,
        hwnd,
        op.as_deref(),
        &f,
        params.as_deref(),
        dir.as_deref(),
        show_cmd,
    )
}

pub fn sh_file_operation_a(ctx: &mut CompatContext, op: &ShFileOp) -> i32 {
    sh_file_operation_w(ctx, op)
}

pub fn sh_create_directory_ex_a(
    ctx: &mut CompatContext,
    hwnd: u64,
    path: &[u8],
    security: u64,
) -> i32 {
    let p = cstr_to_string(path);
    sh_create_directory_ex_w(ctx, hwnd, &p, security)
}

// =========================================================================
// SHGetFileInfo — file type information
// =========================================================================

pub const SHGFI_ICON: u32 = 0x000000100;
pub const SHGFI_DISPLAYNAME: u32 = 0x000000200;
pub const SHGFI_TYPENAME: u32 = 0x000000400;
pub const SHGFI_ATTRIBUTES: u32 = 0x000000800;
pub const SHGFI_ICONLOCATION: u32 = 0x000001000;
pub const SHGFI_EXETYPE: u32 = 0x000002000;
pub const SHGFI_SYSICONINDEX: u32 = 0x000004000;
pub const SHGFI_LINKOVERLAY: u32 = 0x000008000;
pub const SHGFI_SELECTED: u32 = 0x000010000;
pub const SHGFI_LARGEICON: u32 = 0x000000000;
pub const SHGFI_SMALLICON: u32 = 0x000000001;
pub const SHGFI_OPENICON: u32 = 0x000000002;
pub const SHGFI_SHELLICONSIZE: u32 = 0x000000004;
pub const SHGFI_USEFILEATTRIBUTES: u32 = 0x000000010;

pub const SFGAO_FILESYSTEM: u32 = 0x40000000;
pub const SFGAO_FOLDER: u32 = 0x20000000;
pub const SFGAO_HASSUBFOLDER: u32 = 0x80000000;

#[derive(Debug, Clone)]
pub struct ShFileInfo {
    pub icon: u64,
    pub icon_index: i32,
    pub attributes: u32,
    pub display_name: String,
    pub type_name: String,
}

impl ShFileInfo {
    pub fn new() -> Self {
        Self {
            icon: 0,
            icon_index: 0,
            attributes: 0,
            display_name: String::new(),
            type_name: String::new(),
        }
    }
}

fn guess_type_name(path: &str) -> String {
    let ext_pos = path.rfind('.');
    match ext_pos {
        Some(pos) => {
            let ext = &path[pos..];
            let lower: String = ext
                .chars()
                .map(|c| {
                    if c >= 'A' && c <= 'Z' {
                        (c as u8 + 32) as char
                    } else {
                        c
                    }
                })
                .collect();
            match lower.as_str() {
                ".exe" => String::from("Application"),
                ".dll" => String::from("Application Extension"),
                ".txt" => String::from("Text Document"),
                ".doc" | ".docx" => String::from("Microsoft Word Document"),
                ".xls" | ".xlsx" => String::from("Microsoft Excel Worksheet"),
                ".pdf" => String::from("PDF Document"),
                ".jpg" | ".jpeg" => String::from("JPEG Image"),
                ".png" => String::from("PNG Image"),
                ".gif" => String::from("GIF Image"),
                ".bmp" => String::from("Bitmap Image"),
                ".mp3" => String::from("MP3 Audio"),
                ".wav" => String::from("WAV Audio"),
                ".mp4" => String::from("MP4 Video"),
                ".avi" => String::from("AVI Video"),
                ".zip" => String::from("ZIP Archive"),
                ".rar" => String::from("RAR Archive"),
                ".7z" => String::from("7-Zip Archive"),
                ".html" | ".htm" => String::from("HTML Document"),
                ".xml" => String::from("XML Document"),
                ".json" => String::from("JSON File"),
                ".ini" => String::from("Configuration Settings"),
                ".bat" | ".cmd" => String::from("Windows Batch File"),
                ".sys" => String::from("System File"),
                ".lnk" => String::from("Shortcut"),
                _ => {
                    let mut name = String::from(&ext[1..]);
                    name.push_str(" File");
                    name
                }
            }
        }
        None => String::from("File"),
    }
}

fn extract_display_name(path: &str) -> String {
    let pos = path
        .rfind(|c: char| c == '\\' || c == '/')
        .map(|p| p + 1)
        .unwrap_or(0);
    String::from(&path[pos..])
}

pub fn sh_get_file_info_w(
    ctx: &mut CompatContext,
    path: &str,
    file_attributes: u32,
    info: &mut ShFileInfo,
    flags: u32,
) -> u64 {
    if path.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }

    if flags & SHGFI_ICON != 0 {
        info.icon = 0x1C0A0001;
        info.icon_index = 0;
    }

    if flags & SHGFI_DISPLAYNAME != 0 {
        info.display_name = extract_display_name(path);
    }

    if flags & SHGFI_TYPENAME != 0 {
        info.type_name = guess_type_name(path);
    }

    if flags & SHGFI_ATTRIBUTES != 0 {
        info.attributes = SFGAO_FILESYSTEM;
        if file_attributes & 0x10 != 0 {
            info.attributes |= SFGAO_FOLDER | SFGAO_HASSUBFOLDER;
        }
    }

    set_last_error(ctx, ERROR_SUCCESS);
    1
}

pub fn sh_get_file_info_a(
    ctx: &mut CompatContext,
    path: &[u8],
    file_attributes: u32,
    info: &mut ShFileInfo,
    flags: u32,
) -> u64 {
    let p = cstr_to_string(path);
    sh_get_file_info_w(ctx, &p, file_attributes, info, flags)
}

// =========================================================================
// ExtractIcon — return dummy icon handles
// =========================================================================

pub fn extract_icon_w(ctx: &mut CompatContext, _inst: u64, exe_file: &str, icon_index: u32) -> u64 {
    if exe_file.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }
    let _ = icon_index;
    set_last_error(ctx, ERROR_SUCCESS);
    0x1C0A0002
}

pub fn extract_icon_a(ctx: &mut CompatContext, inst: u64, exe_file: &[u8], icon_index: u32) -> u64 {
    let f = cstr_to_string(exe_file);
    extract_icon_w(ctx, inst, &f, icon_index)
}

pub fn extract_icon_ex_w(
    ctx: &mut CompatContext,
    file: &str,
    icon_index: i32,
    large: &mut [u64],
    small: &mut [u64],
    icons: u32,
) -> u32 {
    if file.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }
    let _ = icon_index;
    let count = icons.min(large.len() as u32).min(small.len() as u32);
    for i in 0..count as usize {
        large[i] = 0x1C0A0010 + i as u64;
        small[i] = 0x1C0A0020 + i as u64;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    count
}

// =========================================================================
// CommandLineToArgvW — parse command line into argv array
// =========================================================================

pub fn command_line_to_argv_w(cmd_line: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = cmd_line.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
            }
            '\\' => {
                let mut backslash_count = 1u32;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    backslash_count += 1;
                }
                if chars.peek() == Some(&'"') {
                    for _ in 0..backslash_count / 2 {
                        current.push('\\');
                    }
                    if backslash_count % 2 != 0 {
                        current.push('"');
                        chars.next();
                    }
                } else {
                    for _ in 0..backslash_count {
                        current.push('\\');
                    }
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(core::mem::take(&mut current));
                }
            }
            _ => {
                current.push(c);
            }
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    if args.is_empty() {
        args.push(String::new());
    }

    args
}

// =========================================================================
// Additional known folder GUIDs
// =========================================================================

pub const FOLDERID_MUSIC: [u8; 16] = [
    0x91, 0x0C, 0xD5, 0x4B, 0xEB, 0xF1, 0x4D, 0x12, 0xA5, 0xCE, 0x6C, 0x3F, 0x7B, 0xCC, 0x33, 0x89,
];
pub const FOLDERID_PICTURES: [u8; 16] = [
    0xF4, 0x2E, 0xE2, 0x33, 0x19, 0x06, 0x45, 0xE4, 0xA1, 0x8D, 0x5B, 0x04, 0x7B, 0x5A, 0x08, 0x01,
];
pub const FOLDERID_VIDEOS: [u8; 16] = [
    0xAB, 0x5F, 0xB8, 0x18, 0x01, 0x10, 0x4D, 0x86, 0x90, 0x2C, 0x77, 0x12, 0xC3, 0xF3, 0x35, 0x77,
];
pub const FOLDERID_PROGRAM_FILES: [u8; 16] = [
    0xF3, 0x8B, 0x5A, 0x90, 0x56, 0x45, 0x46, 0x11, 0x96, 0xEF, 0x69, 0xF2, 0x07, 0x29, 0xD6, 0x64,
];
pub const FOLDERID_ROAMING_APPDATA: [u8; 16] = [
    0x55, 0x2C, 0xB0, 0x3E, 0x30, 0xF4, 0x48, 0x82, 0xB3, 0x91, 0xCF, 0x89, 0xE5, 0x7B, 0x47, 0x5C,
];
pub const FOLDERID_LOCAL_APPDATA: [u8; 16] = [
    0xBC, 0x3E, 0xB1, 0xF7, 0xF1, 0xAC, 0x44, 0xEC, 0xA3, 0x44, 0x49, 0xCA, 0x38, 0xCC, 0x2D, 0x77,
];
pub const FOLDERID_PROGRAM_DATA: [u8; 16] = [
    0xAA, 0xE3, 0x62, 0x82, 0x1F, 0xFA, 0x43, 0x9E, 0xAB, 0xD2, 0x0A, 0x53, 0x5F, 0xCC, 0xCE, 0x28,
];

pub fn sh_get_known_folder_path_extended(
    ctx: &mut CompatContext,
    folder_id: &[u8; 16],
    _flags: u32,
    _token: u64,
) -> Option<String> {
    let folder_str = if *folder_id == FOLDERID_DOCUMENTS {
        "C:\\Users\\user\\Documents"
    } else if *folder_id == FOLDERID_DOWNLOADS {
        "C:\\Users\\user\\Downloads"
    } else if *folder_id == FOLDERID_DESKTOP {
        "C:\\Users\\user\\Desktop"
    } else if *folder_id == FOLDERID_PROFILE {
        "C:\\Users\\user"
    } else if *folder_id == FOLDERID_MUSIC {
        "C:\\Users\\user\\Music"
    } else if *folder_id == FOLDERID_PICTURES {
        "C:\\Users\\user\\Pictures"
    } else if *folder_id == FOLDERID_VIDEOS {
        "C:\\Users\\user\\Videos"
    } else if *folder_id == FOLDERID_PROGRAM_FILES {
        "C:\\Program Files"
    } else if *folder_id == FOLDERID_ROAMING_APPDATA {
        "C:\\Users\\user\\AppData\\Roaming"
    } else if *folder_id == FOLDERID_LOCAL_APPDATA {
        "C:\\Users\\user\\AppData\\Local"
    } else if *folder_id == FOLDERID_PROGRAM_DATA {
        "C:\\ProgramData"
    } else {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return None;
    };

    set_last_error(ctx, ERROR_SUCCESS);
    Some(String::from(folder_str))
}

// =========================================================================
// Path mapping: Windows → AthenaOS
// =========================================================================

pub fn map_win_folder_to_athena(win_path: &str) -> String {
    let lower: String = win_path
        .chars()
        .map(|c| {
            if c >= 'A' && c <= 'Z' {
                (c as u8 + 32) as char
            } else {
                c
            }
        })
        .collect();
    let normalized = lower.replace('\\', "/");

    if normalized.starts_with("c:/users/user/desktop") {
        let rest = &win_path[21..];
        let mut result = String::from("/home/user/Desktop");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/documents") {
        let rest = &win_path[23..];
        let mut result = String::from("/home/user/Documents");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/downloads") {
        let rest = &win_path[23..];
        let mut result = String::from("/home/user/Downloads");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/music") {
        let rest = &win_path[19..];
        let mut result = String::from("/home/user/Music");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/pictures") {
        let rest = &win_path[22..];
        let mut result = String::from("/home/user/Pictures");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/videos") {
        let rest = &win_path[20..];
        let mut result = String::from("/home/user/Videos");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/appdata/roaming") {
        let rest = &win_path[29..];
        let mut result = String::from("/home/user/.config");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user/appdata/local") {
        let rest = &win_path[27..];
        let mut result = String::from("/home/user/.local/share");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/users/user") {
        let rest = &win_path[13..];
        let mut result = String::from("/home/user");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/program files") {
        let rest = &win_path[16..];
        let mut result = String::from("/opt");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/programdata") {
        let rest = &win_path[14..];
        let mut result = String::from("/etc/athenaos");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/windows/system32") {
        let rest = &win_path[19..];
        let mut result = String::from("/sys/athenaos/system32");
        result.push_str(rest);
        return result;
    }
    if normalized.starts_with("c:/windows") {
        let rest = &win_path[10..];
        let mut result = String::from("/sys/athenaos");
        result.push_str(rest);
        return result;
    }

    translate_win_path(win_path)
}

// =========================================================================
// ANSI path utilities
// =========================================================================

pub fn path_combine_a(dest: &mut [u8], dir: &[u8], file: &[u8]) -> bool {
    let d = cstr_to_string(dir);
    let f = cstr_to_string(file);

    let combined = if d.is_empty() {
        f
    } else if f.is_empty() {
        d
    } else {
        let mut result = d;
        if !result.ends_with('\\') && !result.ends_with('/') {
            result.push('\\');
        }
        result.push_str(&f);
        result
    };

    if combined.len() + 1 > dest.len() {
        return false;
    }
    string_to_ansi_buf(&combined, dest);
    true
}

pub fn path_find_file_name_a(path: &[u8]) -> String {
    let s = cstr_to_string(path);
    path_find_file_name_w(&s)
}

pub fn path_remove_file_spec_a(path: &mut String) -> bool {
    path_remove_file_spec_w(path)
}

pub fn path_is_directory_a(path: &[u8]) -> bool {
    let s = cstr_to_string(path);
    path_is_directory_w(&s)
}

pub fn path_file_exists_a(path: &[u8]) -> bool {
    let s = cstr_to_string(path);
    path_file_exists_w(&s)
}

pub fn path_add_backslash_a(path: &mut String) {
    path_add_backslash_w(path);
}

pub fn path_remove_extension_a(path: &mut String) {
    path_remove_extension_w(path);
}

pub fn path_remove_extension_w(path: &mut String) {
    let fname_start = path
        .rfind(|c: char| c == '\\' || c == '/')
        .map(|p| p + 1)
        .unwrap_or(0);
    if let Some(dot_pos) = path[fname_start..].rfind('.') {
        path.truncate(fname_start + dot_pos);
    }
}

pub fn path_find_extension_a(path: &[u8]) -> String {
    let s = cstr_to_string(path);
    path_find_extension_w(&s)
}

// =========================================================================
// SHChangeNotify — notify shell of filesystem changes (stub)
// =========================================================================

pub const SHCNE_RENAMEITEM: u32 = 0x00000001;
pub const SHCNE_CREATE: u32 = 0x00000002;
pub const SHCNE_DELETE: u32 = 0x00000004;
pub const SHCNE_MKDIR: u32 = 0x00000008;
pub const SHCNE_RMDIR: u32 = 0x00000010;
pub const SHCNE_UPDATEDIR: u32 = 0x00001000;
pub const SHCNE_UPDATEITEM: u32 = 0x00002000;
pub const SHCNE_ASSOCCHANGED: u32 = 0x08000000;
pub const SHCNE_ALLEVENTS: u32 = 0x7FFFFFFF;

pub const SHCNF_IDLIST: u32 = 0x0000;
pub const SHCNF_PATH: u32 = 0x0001;
pub const SHCNF_FLUSH: u32 = 0x1000;
pub const SHCNF_FLUSHNOWAIT: u32 = 0x3000;

pub fn sh_change_notify(_event_id: u32, _flags: u32, _item1: u64, _item2: u64) {
    // Shell change notifications are no-ops in emulation.
}

// =========================================================================
// IsUserAnAdmin — always returns false for sandboxed apps
// =========================================================================

pub fn is_user_an_admin() -> bool {
    false
}

// =========================================================================
// SHGetDesktopFolder — return opaque interface pointer
// =========================================================================

pub fn sh_get_desktop_folder(ctx: &mut CompatContext) -> u64 {
    set_last_error(ctx, ERROR_SUCCESS);
    0xDE5C0001
}

// =========================================================================
// SHParseDisplayName — return dummy PIDL
// =========================================================================

pub fn sh_parse_display_name(ctx: &mut CompatContext, name: &str, pidl: &mut u64) -> i32 {
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return E_INVALIDARG;
    }
    *pidl = 0x80002000;
    set_last_error(ctx, ERROR_SUCCESS);
    S_OK
}

// =========================================================================
// ILFree — free PIDL (no-op)
// =========================================================================

pub fn il_free(_pidl: u64) {}

// =========================================================================
// SHGetPathFromIDListW
// =========================================================================

pub fn sh_get_path_from_id_list_w(ctx: &mut CompatContext, _pidl: u64, path: &mut [u16]) -> bool {
    let default_path = "C:\\Users\\user\\Desktop";
    let wide = crate::string_to_wide(default_path);
    if wide.len() > path.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return false;
    }
    let copy_len = wide.len().min(path.len());
    path[..copy_len].copy_from_slice(&wide[..copy_len]);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}
