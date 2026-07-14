//! version.dll — File version information API for RaeBridge.
//!
//! Many Windows applications query file version info to check DLL versions,
//! product versions, and company names. This module synthesizes plausible
//! version data for any queried file.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::{wide_to_string, CompatContext, ERROR_INVALID_PARAMETER, ERROR_SUCCESS};

// =========================================================================
// Error / return codes
// =========================================================================

pub const VFT_UNKNOWN: u32 = 0x00000000;
pub const VFT_APP: u32 = 0x00000001;
pub const VFT_DLL: u32 = 0x00000002;
pub const VFT_DRV: u32 = 0x00000003;
pub const VFT_FONT: u32 = 0x00000004;
pub const VFT_VXD: u32 = 0x00000005;
pub const VFT_STATIC_LIB: u32 = 0x00000007;

pub const VFT2_UNKNOWN: u32 = 0x00000000;

pub const VOS_NT_WINDOWS32: u32 = 0x00040004;
pub const VOS__WINDOWS32: u32 = 0x00000004;

pub const VS_FF_DEBUG: u32 = 0x00000001;
pub const VS_FF_PRERELEASE: u32 = 0x00000002;
pub const VS_FF_PATCHED: u32 = 0x00000004;
pub const VS_FF_PRIVATEBUILD: u32 = 0x00000008;
pub const VS_FF_INFOINFERRED: u32 = 0x00000010;
pub const VS_FF_SPECIALBUILD: u32 = 0x00000020;

pub const VS_FFI_SIGNATURE: u32 = 0xFEEF04BD;
pub const VS_FFI_STRUCVERSION: u32 = 0x00010000;

// =========================================================================
// VS_FIXEDFILEINFO structure
// =========================================================================

#[derive(Debug, Clone, Copy)]
pub struct VsFixedFileInfo {
    pub signature: u32,
    pub struc_version: u32,
    pub file_version_ms: u32,
    pub file_version_ls: u32,
    pub product_version_ms: u32,
    pub product_version_ls: u32,
    pub file_flags_mask: u32,
    pub file_flags: u32,
    pub file_os: u32,
    pub file_type: u32,
    pub file_subtype: u32,
    pub file_date_ms: u32,
    pub file_date_ls: u32,
}

impl VsFixedFileInfo {
    pub fn synthetic(major: u16, minor: u16, build: u16, revision: u16) -> Self {
        Self {
            signature: VS_FFI_SIGNATURE,
            struc_version: VS_FFI_STRUCVERSION,
            file_version_ms: ((major as u32) << 16) | minor as u32,
            file_version_ls: ((build as u32) << 16) | revision as u32,
            product_version_ms: ((major as u32) << 16) | minor as u32,
            product_version_ls: ((build as u32) << 16) | revision as u32,
            file_flags_mask: 0x3F,
            file_flags: 0,
            file_os: VOS_NT_WINDOWS32,
            file_type: VFT_DLL,
            file_subtype: VFT2_UNKNOWN,
            file_date_ms: 0,
            file_date_ls: 0,
        }
    }

    pub fn file_version(&self) -> (u16, u16, u16, u16) {
        (
            (self.file_version_ms >> 16) as u16,
            self.file_version_ms as u16,
            (self.file_version_ls >> 16) as u16,
            self.file_version_ls as u16,
        )
    }

    pub fn product_version(&self) -> (u16, u16, u16, u16) {
        (
            (self.product_version_ms >> 16) as u16,
            self.product_version_ms as u16,
            (self.product_version_ls >> 16) as u16,
            self.product_version_ls as u16,
        )
    }
}

impl Default for VsFixedFileInfo {
    fn default() -> Self {
        Self::synthetic(10, 0, 22631, 0)
    }
}

// =========================================================================
// Version info string block
// =========================================================================

#[derive(Debug, Clone)]
pub struct VersionStringEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct VersionInfoBlock {
    pub fixed_info: VsFixedFileInfo,
    pub strings: Vec<VersionStringEntry>,
    pub translation: u32,
}

impl VersionInfoBlock {
    pub fn synthetic_for_file(filename: &str) -> Self {
        let product_name = guess_product_name(filename);
        let (major, minor, build, rev) = guess_version(filename);
        let fixed = VsFixedFileInfo::synthetic(major, minor, build, rev);

        let version_str = format_version(major, minor, build, rev);

        let strings = Vec::from([
            VersionStringEntry {
                key: String::from("CompanyName"),
                value: String::from("Microsoft Corporation"),
            },
            VersionStringEntry {
                key: String::from("FileDescription"),
                value: product_name.clone(),
            },
            VersionStringEntry {
                key: String::from("FileVersion"),
                value: version_str.clone(),
            },
            VersionStringEntry {
                key: String::from("InternalName"),
                value: strip_extension(filename),
            },
            VersionStringEntry {
                key: String::from("LegalCopyright"),
                value: String::from("(c) Microsoft Corporation. All rights reserved."),
            },
            VersionStringEntry {
                key: String::from("OriginalFilename"),
                value: extract_filename(filename),
            },
            VersionStringEntry {
                key: String::from("ProductName"),
                value: String::from("Microsoft Windows Operating System"),
            },
            VersionStringEntry {
                key: String::from("ProductVersion"),
                value: version_str,
            },
        ]);

        Self {
            fixed_info: fixed,
            strings,
            translation: 0x04090000 | 0x04B0,
        }
    }

    pub fn find_string(&self, key: &str) -> Option<&str> {
        self.strings
            .iter()
            .find(|e| ascii_eq_ignore_case(&e.key, key))
            .map(|e| e.value.as_str())
    }

    pub fn serialized_size(&self) -> u32 {
        let mut size: u32 = 52;
        for entry in &self.strings {
            size += 6 + (entry.key.len() as u32 + 1) * 2 + (entry.value.len() as u32 + 1) * 2;
        }
        size += 4;
        size
    }
}

// =========================================================================
// Internal helpers
// =========================================================================

fn ascii_eq_ignore_case(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).all(|(x, y)| {
        let lx = if x >= b'A' && x <= b'Z' { x + 32 } else { x };
        let ly = if y >= b'A' && y <= b'Z' { y + 32 } else { y };
        lx == ly
    })
}

fn extract_filename(path: &str) -> String {
    let pos = path
        .rfind(|c: char| c == '\\' || c == '/')
        .map(|p| p + 1)
        .unwrap_or(0);
    String::from(&path[pos..])
}

fn strip_extension(path: &str) -> String {
    let fname = extract_filename(path);
    match fname.rfind('.') {
        Some(pos) => String::from(&fname[..pos]),
        None => fname,
    }
}

fn guess_product_name(filename: &str) -> String {
    let base = strip_extension(filename);
    let lower = base.to_ascii_lowercase();
    match lower.as_str() {
        "kernel32" => String::from("Windows NT BASE API Client DLL"),
        "user32" => String::from("Multi-User Windows USER API Client DLL"),
        "gdi32" => String::from("GDI Client DLL"),
        "ntdll" => String::from("NT Layer DLL"),
        "advapi32" => String::from("Advanced Windows 32 Base API"),
        "shell32" => String::from("Windows Shell Common Dll"),
        "ole32" => String::from("Microsoft OLE for Windows"),
        "ws2_32" => String::from("Windows Socket 2.0 32-Bit DLL"),
        "comctl32" => String::from("User Experience Controls Library"),
        "msvcrt" => String::from("Windows NT CRT DLL"),
        "xinput1_3" | "xinput1_4" | "xinput9_1_0" => {
            String::from("Microsoft Common Controller API")
        }
        "d3d9" => String::from("Microsoft Direct3D 9"),
        "d3d11" => String::from("Direct3D 11 Runtime"),
        "d3d12" => String::from("Direct3D 12 Runtime"),
        "dxgi" => String::from("DirectX Graphics Infrastructure"),
        "dinput8" => String::from("Microsoft DirectInput"),
        "winmm" => String::from("MCI API DLL"),
        "version" => String::from("Version Checking and File Installation Libraries"),
        "setupapi" => String::from("Windows Setup API"),
        "crypt32" => String::from("Crypto API32"),
        "dbghelp" => String::from("Windows Image Helper"),
        "psapi" => String::from("Process Status Helper"),
        "iphlpapi" => String::from("IP Helper API"),
        "userenv" => String::from("Userenv"),
        "winhttp" => String::from("Windows HTTP Services"),
        "dwmapi" => String::from("Microsoft Desktop Window Manager API"),
        "uxtheme" => String::from("Microsoft UxTheme Library"),
        "dwrite" => String::from("Microsoft DirectWrite"),
        "shlwapi" => String::from("Shell Light-Weight Utility Library"),
        _ => {
            let mut desc = base;
            desc.push_str(" Module");
            desc
        }
    }
}

fn guess_version(filename: &str) -> (u16, u16, u16, u16) {
    let base = strip_extension(filename);
    let lower = base.to_ascii_lowercase();

    if lower.starts_with("d3d") || lower.starts_with("dxgi") {
        return (10, 0, 22631, 4169);
    }
    if lower.starts_with("xinput") {
        return (1, 4, 22631, 0);
    }
    if lower == "winmm" || lower == "version" || lower == "shlwapi" {
        return (10, 0, 22631, 0);
    }

    (10, 0, 22631, 0)
}

fn format_version(major: u16, minor: u16, build: u16, revision: u16) -> String {
    let mut s = String::new();
    push_u16(&mut s, major);
    s.push('.');
    push_u16(&mut s, minor);
    s.push('.');
    push_u16(&mut s, build);
    s.push('.');
    push_u16(&mut s, revision);
    s
}

fn push_u16(s: &mut String, val: u16) {
    if val == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 5];
    let mut n = val;
    let mut i = 5;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    for &b in &buf[i..] {
        s.push(b as char);
    }
}

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

// =========================================================================
// GetFileVersionInfoSizeA/W
// =========================================================================

pub fn get_file_version_info_size_w(
    ctx: &mut CompatContext,
    filename: &[u16],
    handle: &mut u32,
) -> u32 {
    let name = wide_to_string(filename);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }

    *handle = 0;
    let block = VersionInfoBlock::synthetic_for_file(&name);
    let size = block.serialized_size();
    set_last_error(ctx, ERROR_SUCCESS);
    size
}

pub fn get_file_version_info_size_a(
    ctx: &mut CompatContext,
    filename: &[u8],
    handle: &mut u32,
) -> u32 {
    let name = cstr_to_string(filename);
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    get_file_version_info_size_w(ctx, &wide, handle)
}

// =========================================================================
// GetFileVersionInfoA/W
// =========================================================================

pub fn get_file_version_info_w(
    ctx: &mut CompatContext,
    filename: &[u16],
    _handle: u32,
    _len: u32,
    data: &mut VersionInfoBlock,
) -> bool {
    let name = wide_to_string(filename);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return false;
    }

    *data = VersionInfoBlock::synthetic_for_file(&name);
    set_last_error(ctx, ERROR_SUCCESS);
    true
}

pub fn get_file_version_info_a(
    ctx: &mut CompatContext,
    filename: &[u8],
    handle: u32,
    len: u32,
    data: &mut VersionInfoBlock,
) -> bool {
    let name = cstr_to_string(filename);
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    get_file_version_info_w(ctx, &wide, handle, len, data)
}

// =========================================================================
// VerQueryValueA/W
// =========================================================================

#[derive(Debug, Clone)]
pub enum VerQueryResult {
    FixedInfo(VsFixedFileInfo),
    StringValue(String),
    Translation(u32),
}

pub fn ver_query_value_w(
    ctx: &mut CompatContext,
    block: &VersionInfoBlock,
    sub_block: &[u16],
) -> Option<VerQueryResult> {
    let query = wide_to_string(sub_block);
    ver_query_value_impl(ctx, block, &query)
}

pub fn ver_query_value_a(
    ctx: &mut CompatContext,
    block: &VersionInfoBlock,
    sub_block: &[u8],
) -> Option<VerQueryResult> {
    let query = cstr_to_string(sub_block);
    ver_query_value_impl(ctx, block, &query)
}

fn ver_query_value_impl(
    ctx: &mut CompatContext,
    block: &VersionInfoBlock,
    query: &str,
) -> Option<VerQueryResult> {
    let normalized = query.replace('/', "\\");

    if normalized == "\\" || normalized.is_empty() {
        set_last_error(ctx, ERROR_SUCCESS);
        return Some(VerQueryResult::FixedInfo(block.fixed_info));
    }

    if ascii_eq_ignore_case(&normalized, "\\VarFileInfo\\Translation") {
        set_last_error(ctx, ERROR_SUCCESS);
        return Some(VerQueryResult::Translation(block.translation));
    }

    if let Some(rest) = strip_prefix_icase(&normalized, "\\StringFileInfo\\") {
        let key = if let Some(after_lang) = rest.find('\\').map(|p| &rest[p + 1..]) {
            after_lang
        } else {
            rest
        };

        if let Some(value) = block.find_string(key) {
            set_last_error(ctx, ERROR_SUCCESS);
            return Some(VerQueryResult::StringValue(String::from(value)));
        }
    }

    set_last_error(ctx, ERROR_INVALID_PARAMETER);
    None
}

fn strip_prefix_icase<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() {
        return None;
    }
    let candidate = &s[..prefix.len()];
    if ascii_eq_ignore_case(candidate, prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

// =========================================================================
// VerFindFileA/W (stubs)
// =========================================================================

pub fn ver_find_file_w(
    _ctx: &mut CompatContext,
    _flags: u32,
    _filename: &[u16],
    _win_dir: &[u16],
    _app_dir: &[u16],
    cur_dir: &mut [u16],
    cur_dir_len: &mut u32,
    dest_dir: &mut [u16],
    dest_dir_len: &mut u32,
) -> u32 {
    if !cur_dir.is_empty() {
        cur_dir[0] = 0;
    }
    *cur_dir_len = 0;
    if !dest_dir.is_empty() {
        dest_dir[0] = 0;
    }
    *dest_dir_len = 0;
    0
}

pub fn ver_find_file_a(
    _ctx: &mut CompatContext,
    _flags: u32,
    _filename: &[u8],
    _win_dir: &[u8],
    _app_dir: &[u8],
    cur_dir: &mut [u8],
    cur_dir_len: &mut u32,
    dest_dir: &mut [u8],
    dest_dir_len: &mut u32,
) -> u32 {
    if !cur_dir.is_empty() {
        cur_dir[0] = 0;
    }
    *cur_dir_len = 0;
    if !dest_dir.is_empty() {
        dest_dir[0] = 0;
    }
    *dest_dir_len = 0;
    0
}

// =========================================================================
// VerInstallFileA/W (stubs — return success)
// =========================================================================

pub fn ver_install_file_w(
    _ctx: &mut CompatContext,
    _flags: u32,
    _src_filename: &[u16],
    _dst_filename: &[u16],
    _src_dir: &[u16],
    _dst_dir: &[u16],
    _cur_dir: &[u16],
    _tmp_file: &mut [u16],
    _tmp_file_len: &mut u32,
) -> u32 {
    0
}

pub fn ver_install_file_a(
    _ctx: &mut CompatContext,
    _flags: u32,
    _src_filename: &[u8],
    _dst_filename: &[u8],
    _src_dir: &[u8],
    _dst_dir: &[u8],
    _cur_dir: &[u8],
    _tmp_file: &mut [u8],
    _tmp_file_len: &mut u32,
) -> u32 {
    0
}

// =========================================================================
// GetFileVersionInfoExA/W (extended variants — same behavior)
// =========================================================================

pub fn get_file_version_info_ex_w(
    ctx: &mut CompatContext,
    _flags: u32,
    filename: &[u16],
    handle: u32,
    len: u32,
    data: &mut VersionInfoBlock,
) -> bool {
    get_file_version_info_w(ctx, filename, handle, len, data)
}

pub fn get_file_version_info_size_ex_w(
    ctx: &mut CompatContext,
    _flags: u32,
    filename: &[u16],
    handle: &mut u32,
) -> u32 {
    get_file_version_info_size_w(ctx, filename, handle)
}

// =========================================================================
// VerLanguageNameA/W
// =========================================================================

pub fn ver_language_name_w(lang_id: u32, lang_name: &mut String) -> u32 {
    lang_name.clear();
    match lang_id & 0x3FF {
        0x009 => lang_name.push_str("English"),
        0x00C => lang_name.push_str("French"),
        0x007 => lang_name.push_str("German"),
        0x011 => lang_name.push_str("Japanese"),
        0x004 => lang_name.push_str("Chinese"),
        0x00A => lang_name.push_str("Spanish"),
        0x010 => lang_name.push_str("Italian"),
        0x016 => lang_name.push_str("Portuguese"),
        0x019 => lang_name.push_str("Russian"),
        0x012 => lang_name.push_str("Korean"),
        0x01D => lang_name.push_str("Swedish"),
        0x013 => lang_name.push_str("Dutch"),
        _ => lang_name.push_str("Language Neutral"),
    }
    lang_name.len() as u32
}

pub fn ver_language_name_a(lang_id: u32, lang_name: &mut String) -> u32 {
    ver_language_name_w(lang_id, lang_name)
}

// =========================================================================
// ANSI c-string helper
// =========================================================================

fn cstr_to_string(ptr: &[u8]) -> String {
    let end = ptr.iter().position(|&b| b == 0).unwrap_or(ptr.len());
    let mut s = String::new();
    for &b in &ptr[..end] {
        s.push(b as char);
    }
    s
}
