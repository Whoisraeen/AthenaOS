//! shlwapi.dll — Shell Lightweight Utility API: path manipulation, string
//! helpers, URL functions, registry utilities, and color conversion for
//! RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::{
    CompatContext, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER,
    ERROR_SUCCESS,
};

// =========================================================================
// HResult values
// =========================================================================

pub const S_OK: i32 = 0;
pub const S_FALSE: i32 = 1;
pub const E_FAIL: i32 = -2147467259;
pub const E_INVALIDARG: i32 = -2147024809;
pub const E_OUTOFMEMORY: i32 = -2147024882;

// =========================================================================
// Path character type constants
// =========================================================================

pub const GCT_INVALID: u32 = 0x0000;
pub const GCT_LFNCHAR: u32 = 0x0001;
pub const GCT_SHORTCHAR: u32 = 0x0002;
pub const GCT_WILD: u32 = 0x0004;
pub const GCT_SEPARATOR: u32 = 0x0008;

// =========================================================================
// URL scheme constants
// =========================================================================

pub const URL_SCHEME_HTTP: u32 = 2;
pub const URL_SCHEME_HTTPS: u32 = 3;
pub const URL_SCHEME_FTP: u32 = 4;
pub const URL_SCHEME_FILE: u32 = 8;
pub const URL_SCHEME_MAILTO: u32 = 6;
pub const URL_SCHEME_UNKNOWN: u32 = 0xFFFFFFFF;

// =========================================================================
// URL part constants
// =========================================================================

pub const URL_PART_SCHEME: u32 = 1;
pub const URL_PART_HOSTNAME: u32 = 2;
pub const URL_PART_USERNAME: u32 = 3;
pub const URL_PART_PASSWORD: u32 = 4;
pub const URL_PART_PORT: u32 = 5;
pub const URL_PART_QUERY: u32 = 6;

// =========================================================================
// StrFormatByteSize flags
// =========================================================================

pub const SFBS_FLAGS_ROUND_TO_NEAREST_DISPLAYED_DIGIT: u32 = 0x0001;
pub const SFBS_FLAGS_TRUNCATE_UNDISPLAYED_DECIMAL_DIGITS: u32 = 0x0002;

// =========================================================================
// AssocQueryString types
// =========================================================================

pub const ASSOCSTR_COMMAND: u32 = 1;
pub const ASSOCSTR_EXECUTABLE: u32 = 2;
pub const ASSOCSTR_FRIENDLYDOCNAME: u32 = 3;
pub const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
pub const ASSOCSTR_CONTENTTYPE: u32 = 6;

pub const ASSOCF_NONE: u32 = 0x00000000;
pub const ASSOCF_INIT_NOREMAPCLSID: u32 = 0x00000001;

// =========================================================================
// Registry key roots for SHReg functions
// =========================================================================

pub const SHREGSET_HKCU: u32 = 0x00000001;
pub const SHREGSET_FORCE_HKCU: u32 = 0x00000002;
pub const SHREGSET_HKLM: u32 = 0x00000004;
pub const SHREGSET_FORCE_HKLM: u32 = 0x00000008;
pub const SHREGSET_DEFAULT: u32 = SHREGSET_FORCE_HKCU | SHREGSET_HKLM;

// =========================================================================
// PathMatchSpecEx flags
// =========================================================================

pub const PMSF_NORMAL: u32 = 0x00000000;
pub const PMSF_MULTIPLE: u32 = 0x00000001;
pub const PMSF_DONT_STRIP_SPACES: u32 = 0x00010000;

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

fn is_separator(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

fn find_extension(path: &str) -> Option<usize> {
    let fname_start = path
        .rfind(|c: char| c == '\\' || c == '/')
        .map(|p| p + 1)
        .unwrap_or(0);
    path[fname_start..].rfind('.').map(|p| fname_start + p)
}

fn find_filename(path: &str) -> usize {
    path.rfind(|c: char| c == '\\' || c == '/')
        .map(|p| p + 1)
        .unwrap_or(0)
}

// =========================================================================
// Path Functions
// =========================================================================

pub fn path_add_backslash_w(path: &mut String) {
    if !path.is_empty() && !path.ends_with('\\') && !path.ends_with('/') {
        path.push('\\');
    }
}

pub fn path_add_extension_w(path: &mut String, ext: &str) -> bool {
    if find_extension(path).is_some() {
        return false;
    }
    if !ext.starts_with('.') {
        path.push('.');
    }
    path.push_str(ext);
    true
}

pub fn path_append_w(path: &mut String, more: &str) -> bool {
    if more.is_empty() {
        return true;
    }
    if !path.is_empty() && !path.ends_with('\\') && !path.ends_with('/') {
        path.push('\\');
    }
    path.push_str(more);
    true
}

pub fn path_build_root_w(root: &mut String, drive: i32) -> bool {
    if !(0..26).contains(&drive) {
        return false;
    }
    root.clear();
    root.push((b'A' + drive as u8) as char);
    root.push_str(":\\");
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

pub fn path_combine_w(dest: &mut String, dir: &str, file: &str) -> bool {
    dest.clear();
    if dir.is_empty() && file.is_empty() {
        return false;
    }
    if file.len() >= 2 && file.as_bytes()[1] == b':' {
        dest.push_str(file);
        return true;
    }
    if file.starts_with('\\') || file.starts_with('/') {
        dest.push_str(file);
        return true;
    }
    dest.push_str(dir);
    if !dir.is_empty() && !dir.ends_with('\\') && !dir.ends_with('/') {
        dest.push('\\');
    }
    dest.push_str(file);
    true
}

pub fn path_common_prefix_w(path1: &str, path2: &str) -> usize {
    let b1 = path1.as_bytes();
    let b2 = path2.as_bytes();
    let mut last_sep = 0;
    let limit = b1.len().min(b2.len());
    for i in 0..limit {
        let c1 = if b1[i] >= b'A' && b1[i] <= b'Z' {
            b1[i] + 32
        } else {
            b1[i]
        };
        let c2 = if b2[i] >= b'A' && b2[i] <= b'Z' {
            b2[i] + 32
        } else {
            b2[i]
        };
        if c1 != c2 {
            break;
        }
        if is_separator(b1[i]) {
            last_sep = i + 1;
        }
        if i + 1 == limit {
            last_sep = limit;
        }
    }
    last_sep
}

pub fn path_compact_path_w(path: &str, max_chars: u32) -> String {
    if path.len() <= max_chars as usize {
        return String::from(path);
    }
    let fname = &path[find_filename(path)..];
    let needed = 4 + fname.len(); // "X:\..." + filename
    if needed > max_chars as usize {
        return String::from(fname);
    }
    let prefix_len = max_chars as usize - fname.len() - 3;
    let mut result = String::from(&path[..prefix_len]);
    result.push_str("...");
    result.push_str(fname);
    result
}

pub fn path_compact_path_ex_w(dest: &mut String, path: &str, max_chars: u32, _flags: u32) -> bool {
    *dest = path_compact_path_w(path, max_chars);
    true
}

pub fn path_create_from_url_w(url: &str, path: &mut String) -> i32 {
    if let Some(rest) = url.strip_prefix("file:///") {
        path.clear();
        let decoded = rest.replace('/', "\\");
        path.push_str(&decoded);
        S_OK
    } else {
        E_INVALIDARG
    }
}

pub fn path_file_exists_w(_path: &str) -> bool {
    false
}

pub fn path_find_extension_w(path: &str) -> String {
    match find_extension(path) {
        Some(pos) => String::from(&path[pos..]),
        None => String::new(),
    }
}

pub fn path_find_file_name_w(path: &str) -> String {
    String::from(&path[find_filename(path)..])
}

pub fn path_find_next_component_w(path: &str) -> String {
    if let Some(pos) = path.find(|c: char| c == '\\' || c == '/') {
        String::from(&path[pos + 1..])
    } else {
        String::new()
    }
}

pub fn path_find_on_path_w(_file: &str, _dirs: &[&str]) -> Option<String> {
    None
}

pub fn path_find_suffix_array_w(path: &str, suffixes: &[&str]) -> Option<String> {
    for suffix in suffixes {
        if path.ends_with(suffix) {
            return Some(String::from(*suffix));
        }
    }
    None
}

pub fn path_get_args_w(path: &str) -> String {
    if let Some(pos) = path.find(' ') {
        String::from(path[pos..].trim_start())
    } else {
        String::new()
    }
}

pub fn path_get_char_type_w(ch: u16) -> u32 {
    let c = ch as u8;
    match c {
        b'\\' | b'/' => GCT_SEPARATOR,
        b'*' | b'?' => GCT_WILD,
        b'<' | b'>' | b'"' | b'|' => GCT_INVALID,
        0..=31 => GCT_INVALID,
        _ => GCT_LFNCHAR | GCT_SHORTCHAR,
    }
}

pub fn path_get_drive_number_w(path: &str) -> i32 {
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let c = path.as_bytes()[0];
        let lower = if c >= b'A' && c <= b'Z' { c + 32 } else { c };
        if lower >= b'a' && lower <= b'z' {
            return (lower - b'a') as i32;
        }
    }
    -1
}

pub fn path_is_content_type_w(_path: &str, _content_type: &str) -> bool {
    false
}

pub fn path_is_directory_empty_w(_path: &str) -> bool {
    true
}

pub fn path_is_directory_w(_path: &str) -> bool {
    false
}

pub fn path_is_file_spec_w(path: &str) -> bool {
    !path.contains('\\') && !path.contains('/')
}

pub fn path_is_lfn_file_spec_w(path: &str) -> bool {
    let fname = &path[find_filename(path)..];
    fname.len() > 12
}

pub fn path_is_network_path_w(path: &str) -> bool {
    path.starts_with("\\\\")
}

pub fn path_is_prefix_w(prefix: &str, path: &str) -> bool {
    let p = prefix.to_ascii_lowercase();
    let s = path.to_ascii_lowercase();
    s.starts_with(&p)
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

pub fn path_is_root_w(path: &str) -> bool {
    if path == "\\" || path == "/" {
        return true;
    }
    if path.len() == 3 && path.as_bytes()[1] == b':' && is_separator(path.as_bytes()[2]) {
        return true;
    }
    false
}

pub fn path_is_same_root_w(path1: &str, path2: &str) -> bool {
    path_get_drive_number_w(path1) == path_get_drive_number_w(path2)
        && path_get_drive_number_w(path1) >= 0
}

pub fn path_is_system_folder_w(_path: &str, _attrs: u32) -> bool {
    false
}

pub fn path_is_unc_w(path: &str) -> bool {
    path.starts_with("\\\\") && !path.starts_with("\\\\?\\") && !path.starts_with("\\\\.\\")
}

pub fn path_is_unc_server_w(path: &str) -> bool {
    if !path.starts_with("\\\\") {
        return false;
    }
    let rest = &path[2..];
    !rest.contains('\\')
}

pub fn path_is_unc_server_share_w(path: &str) -> bool {
    if !path.starts_with("\\\\") {
        return false;
    }
    let rest = &path[2..];
    let parts: Vec<&str> = rest.split('\\').collect();
    parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty()
}

pub fn path_is_url_w(path: &str) -> bool {
    path.contains("://")
}

pub fn path_make_pretty_w(path: &mut String) -> bool {
    *path = path.to_ascii_lowercase();
    true
}

pub fn path_make_system_folder_w(_path: &str) -> bool {
    true
}

pub fn path_match_spec_w(path: &str, spec: &str) -> bool {
    if spec == "*" || spec == "*.*" {
        return true;
    }
    if let Some(ext_spec) = spec.strip_prefix("*.") {
        return path.ends_with(&alloc::format!(".{}", ext_spec));
    }
    path == spec
}

pub fn path_match_spec_ex_w(path: &str, spec: &str, flags: u32) -> i32 {
    if flags & PMSF_MULTIPLE != 0 {
        for pattern in spec.split(';') {
            if path_match_spec_w(path, pattern.trim()) {
                return S_OK;
            }
        }
        return S_FALSE;
    }
    if path_match_spec_w(path, spec) {
        S_OK
    } else {
        S_FALSE
    }
}

pub fn path_parse_icon_location_w(path: &mut String) -> i32 {
    if let Some(pos) = path.rfind(',') {
        let icon_str = path[pos + 1..].trim();
        let icon_index = icon_str.parse::<i32>().unwrap_or(0);
        path.truncate(pos);
        icon_index
    } else {
        0
    }
}

pub fn path_quote_spaces_w(path: &mut String) {
    if path.contains(' ') && !path.starts_with('"') {
        let mut quoted = String::from("\"");
        quoted.push_str(path);
        quoted.push('"');
        *path = quoted;
    }
}

pub fn path_relative_path_to_w(
    dest: &mut String,
    from: &str,
    from_attrs: u32,
    to: &str,
    _to_attrs: u32,
) -> bool {
    let _ = from_attrs;
    dest.clear();
    let from_parts: Vec<&str> = from
        .split(|c: char| c == '\\' || c == '/')
        .filter(|s| !s.is_empty())
        .collect();
    let to_parts: Vec<&str> = to
        .split(|c: char| c == '\\' || c == '/')
        .filter(|s| !s.is_empty())
        .collect();

    let mut common = 0;
    for i in 0..from_parts.len().min(to_parts.len()) {
        if from_parts[i].eq_ignore_ascii_case(to_parts[i]) {
            common = i + 1;
        } else {
            break;
        }
    }

    for _ in common..from_parts.len() {
        if !dest.is_empty() {
            dest.push('\\');
        }
        dest.push_str("..");
    }
    for part in &to_parts[common..] {
        if !dest.is_empty() {
            dest.push('\\');
        }
        dest.push_str(part);
    }
    if dest.is_empty() {
        dest.push('.');
    }
    true
}

pub fn path_remove_args_w(path: &mut String) {
    if let Some(pos) = path.find(' ') {
        path.truncate(pos);
    }
}

pub fn path_remove_backslash_w(path: &mut String) {
    if path.len() > 1 && (path.ends_with('\\') || path.ends_with('/')) {
        path.pop();
    }
}

pub fn path_remove_blanks_w(path: &mut String) {
    *path = String::from(path.trim());
}

pub fn path_remove_extension_w(path: &mut String) {
    if let Some(pos) = find_extension(path) {
        path.truncate(pos);
    }
}

pub fn path_remove_file_spec_w(path: &mut String) -> bool {
    if let Some(pos) = path.rfind(|c: char| c == '\\' || c == '/') {
        path.truncate(pos);
        true
    } else {
        false
    }
}

pub fn path_rename_extension_w(path: &mut String, ext: &str) -> bool {
    path_remove_extension_w(path);
    if !ext.starts_with('.') {
        path.push('.');
    }
    path.push_str(ext);
    true
}

pub fn path_search_and_qualify_w(path: &str, buf: &mut String) -> bool {
    buf.clear();
    if path_is_relative_w(path) {
        buf.push_str("C:\\");
        buf.push_str(path);
    } else {
        buf.push_str(path);
    }
    true
}

pub fn path_set_dlg_item_path_w(_hwnd: u64, _id: i32, _path: &str) {
    // UI stub — no-op in emulation
}

pub fn path_skip_root_w(path: &str) -> String {
    if path.len() >= 3 && path.as_bytes()[1] == b':' && is_separator(path.as_bytes()[2]) {
        return String::from(&path[3..]);
    }
    if path.starts_with("\\\\") {
        let rest = &path[2..];
        if let Some(pos) = rest.find('\\') {
            if let Some(pos2) = rest[pos + 1..].find('\\') {
                return String::from(&rest[pos + 1 + pos2 + 1..]);
            }
        }
    }
    String::from(path)
}

pub fn path_strip_path_w(path: &mut String) {
    let fname_start = find_filename(path);
    let fname = String::from(&path[fname_start..]);
    *path = fname;
}

pub fn path_strip_to_root_w(path: &mut String) -> bool {
    if path.len() >= 3 && path.as_bytes()[1] == b':' && is_separator(path.as_bytes()[2]) {
        path.truncate(3);
        return true;
    }
    false
}

pub fn path_undecorate_w(path: &mut String) {
    if let Some(start) = path.rfind('[') {
        if path.ends_with(']') {
            path.truncate(start);
            *path = String::from(path.trim_end());
        }
    }
}

pub fn path_un_expand_env_strings_w(path: &str, buf: &mut String) -> bool {
    buf.clear();
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("c:\\users\\user") {
        buf.push_str("%USERPROFILE%");
        buf.push_str(&path[13..]);
        return true;
    }
    if lower.starts_with("c:\\windows") {
        buf.push_str("%SystemRoot%");
        buf.push_str(&path[10..]);
        return true;
    }
    buf.push_str(path);
    false
}

pub fn path_unmake_system_folder_w(_path: &str) -> bool {
    true
}

pub fn path_unquote_spaces_w(path: &mut String) {
    if path.starts_with('"') && path.ends_with('"') && path.len() >= 2 {
        *path = String::from(&path[1..path.len() - 1]);
    }
}

// =========================================================================
// String Functions
// =========================================================================

pub fn str_cat_w(dest: &mut String, src: &str) -> bool {
    dest.push_str(src);
    true
}

pub fn str_cat_buff_w(dest: &mut String, src: &str, max_len: usize) -> bool {
    let remaining = max_len.saturating_sub(dest.len());
    let append = if src.len() > remaining {
        &src[..remaining]
    } else {
        src
    };
    dest.push_str(append);
    true
}

pub fn str_chr_w(s: &str, ch: char) -> Option<usize> {
    s.find(ch)
}

pub fn str_chr_i_w(s: &str, ch: char) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    lower.find(ch.to_ascii_lowercase())
}

pub fn str_cmp_w(a: &str, b: &str) -> i32 {
    match a.cmp(b) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

pub fn str_cmp_i_w(a: &str, b: &str) -> i32 {
    let al = a.to_ascii_lowercase();
    let bl = b.to_ascii_lowercase();
    str_cmp_w(&al, &bl)
}

pub fn str_cmp_logical_w(a: &str, b: &str) -> i32 {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    let mut ai = 0;
    let mut bi = 0;

    while ai < ab.len() && bi < bb.len() {
        let a_digit = ab[ai].is_ascii_digit();
        let b_digit = bb[bi].is_ascii_digit();

        if a_digit && b_digit {
            let mut an: u64 = 0;
            while ai < ab.len() && ab[ai].is_ascii_digit() {
                an = an * 10 + (ab[ai] - b'0') as u64;
                ai += 1;
            }
            let mut bn: u64 = 0;
            while bi < bb.len() && bb[bi].is_ascii_digit() {
                bn = bn * 10 + (bb[bi] - b'0') as u64;
                bi += 1;
            }
            if an != bn {
                return if an < bn { -1 } else { 1 };
            }
        } else {
            let ca = if ab[ai] >= b'A' && ab[ai] <= b'Z' {
                ab[ai] + 32
            } else {
                ab[ai]
            };
            let cb = if bb[bi] >= b'A' && bb[bi] <= b'Z' {
                bb[bi] + 32
            } else {
                bb[bi]
            };
            if ca != cb {
                return if ca < cb { -1 } else { 1 };
            }
            ai += 1;
            bi += 1;
        }
    }

    if ai < ab.len() {
        1
    } else if bi < bb.len() {
        -1
    } else {
        0
    }
}

pub fn str_cmp_n_w(a: &str, b: &str, n: usize) -> i32 {
    let a_sub = if a.len() > n { &a[..n] } else { a };
    let b_sub = if b.len() > n { &b[..n] } else { b };
    str_cmp_w(a_sub, b_sub)
}

pub fn str_cmp_n_i_w(a: &str, b: &str, n: usize) -> i32 {
    let a_sub = if a.len() > n { &a[..n] } else { a };
    let b_sub = if b.len() > n { &b[..n] } else { b };
    str_cmp_i_w(a_sub, b_sub)
}

pub fn str_cpy_w(dest: &mut String, src: &str) {
    dest.clear();
    dest.push_str(src);
}

pub fn str_cpy_n_w(dest: &mut String, src: &str, max_len: usize) {
    dest.clear();
    let copy = if src.len() > max_len {
        &src[..max_len]
    } else {
        src
    };
    dest.push_str(copy);
}

pub fn str_dup_w(s: &str) -> String {
    String::from(s)
}

pub fn str_format_byte_size_w(bytes: u64, buf: &mut String) {
    buf.clear();
    if bytes < 1024 {
        buf.push_str(&alloc::format!("{} bytes", bytes));
    } else if bytes < 1024 * 1024 {
        buf.push_str(&alloc::format!("{:.1} KB", bytes as f64 / 1024.0));
    } else if bytes < 1024 * 1024 * 1024 {
        buf.push_str(&alloc::format!(
            "{:.1} MB",
            bytes as f64 / (1024.0 * 1024.0)
        ));
    } else if bytes < 1024 * 1024 * 1024 * 1024 {
        buf.push_str(&alloc::format!(
            "{:.1} GB",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0)
        ));
    } else {
        buf.push_str(&alloc::format!(
            "{:.1} TB",
            bytes as f64 / (1024.0 * 1024.0 * 1024.0 * 1024.0)
        ));
    }
}

pub fn str_format_byte_size_64_w(bytes: i64, buf: &mut String) {
    str_format_byte_size_w(bytes as u64, buf);
}

pub fn str_format_kb_size_w(bytes: i64, buf: &mut String) {
    buf.clear();
    let kb = (bytes + 1023) / 1024;
    buf.push_str(&alloc::format!("{} KB", kb));
}

pub fn str_from_time_interval_w(ms: u32, digits: u32, buf: &mut String) {
    buf.clear();
    let _ = digits;
    if ms >= 3_600_000 {
        buf.push_str(&alloc::format!("{} hr", ms / 3_600_000));
    } else if ms >= 60_000 {
        buf.push_str(&alloc::format!("{} min", ms / 60_000));
    } else {
        buf.push_str(&alloc::format!("{} sec", ms / 1000));
    }
}

pub fn str_is_intl_equal_w(case_sensitive: bool, a: &str, b: &str, n: i32) -> bool {
    let limit = n as usize;
    let a_sub = if a.len() > limit { &a[..limit] } else { a };
    let b_sub = if b.len() > limit { &b[..limit] } else { b };
    if case_sensitive {
        a_sub == b_sub
    } else {
        a_sub.eq_ignore_ascii_case(b_sub)
    }
}

pub fn str_n_cat_w(dest: &mut String, src: &str, max_append: usize) {
    let append = if src.len() > max_append {
        &src[..max_append]
    } else {
        src
    };
    dest.push_str(append);
}

pub fn str_p_brk_w(s: &str, charset: &str) -> Option<usize> {
    s.find(|c: char| charset.contains(c))
}

pub fn str_r_chr_w(s: &str, ch: char) -> Option<usize> {
    s.rfind(ch)
}

pub fn str_r_chr_i_w(s: &str, ch: char) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    lower.rfind(ch.to_ascii_lowercase())
}

pub fn str_ret_to_buf_w(ret_str: &str, buf: &mut String) -> i32 {
    buf.clear();
    buf.push_str(ret_str);
    S_OK
}

pub fn str_r_str_i_w(source: &str, end: Option<usize>, search: &str) -> Option<usize> {
    let haystack = match end {
        Some(e) => &source[..e.min(source.len())],
        None => source,
    };
    let h_lower = haystack.to_ascii_lowercase();
    let s_lower = search.to_ascii_lowercase();
    h_lower.rfind(&s_lower)
}

pub fn str_spn_w(s: &str, charset: &str) -> usize {
    s.chars().take_while(|c| charset.contains(*c)).count()
}

pub fn str_str_w(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

pub fn str_str_i_w(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.find(&n)
}

pub fn str_str_n_w(haystack: &str, needle: &str, max_chars: u32) -> Option<usize> {
    let limit = (max_chars as usize).min(haystack.len());
    haystack[..limit].find(needle)
}

pub fn str_str_n_i_w(haystack: &str, needle: &str, max_chars: u32) -> Option<usize> {
    let limit = (max_chars as usize).min(haystack.len());
    let h = haystack[..limit].to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.find(&n)
}

pub fn str_to_int_w(s: &str) -> i32 {
    s.trim().parse::<i32>().unwrap_or(0)
}

pub fn str_to_int_64_ex_w(s: &str, result: &mut i64) -> bool {
    match s.trim().parse::<i64>() {
        Ok(v) => {
            *result = v;
            true
        }
        Err(_) => false,
    }
}

pub fn str_to_int_ex_w(s: &str, result: &mut i32) -> bool {
    match s.trim().parse::<i32>() {
        Ok(v) => {
            *result = v;
            true
        }
        Err(_) => false,
    }
}

pub fn str_trim_w(s: &mut String, trim_chars: &str) -> bool {
    let orig_len = s.len();
    let trimmed: String = s.chars().filter(|c| !trim_chars.contains(*c)).collect();
    *s = trimmed;
    s.len() != orig_len
}

pub fn wvnsprintf_w(buf: &mut String, _max: i32, fmt: &str, args: &[&str]) -> i32 {
    buf.clear();
    let mut result = String::from(fmt);
    for arg in args {
        if let Some(pos) = result.find("%s") {
            result.replace_range(pos..pos + 2, arg);
        }
    }
    buf.push_str(&result);
    buf.len() as i32
}

// =========================================================================
// URL Functions
// =========================================================================

pub fn url_apply_scheme_w(url: &str, out: &mut String) -> i32 {
    out.clear();
    if url.contains("://") {
        out.push_str(url);
    } else if url.starts_with("www.") {
        out.push_str("http://");
        out.push_str(url);
    } else {
        out.push_str("http://");
        out.push_str(url);
    }
    S_OK
}

pub fn url_canonicalize_w(url: &str, out: &mut String, _flags: u32) -> i32 {
    out.clear();
    out.push_str(url);
    S_OK
}

pub fn url_combine_w(base: &str, relative: &str, out: &mut String, _flags: u32) -> i32 {
    out.clear();
    if relative.contains("://") {
        out.push_str(relative);
        return S_OK;
    }
    if relative.starts_with('/') {
        if let Some(scheme_end) = base.find("://") {
            let after_scheme = &base[scheme_end + 3..];
            let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
            out.push_str(&base[..scheme_end + 3 + host_end]);
            out.push_str(relative);
        } else {
            out.push_str(relative);
        }
        return S_OK;
    }
    if let Some(last_slash) = base.rfind('/') {
        out.push_str(&base[..last_slash + 1]);
    } else {
        out.push_str(base);
        out.push('/');
    }
    out.push_str(relative);
    S_OK
}

pub fn url_compare_w(url1: &str, url2: &str, ignore_slash: bool) -> i32 {
    let a = if ignore_slash {
        url1.trim_end_matches('/')
    } else {
        url1
    };
    let b = if ignore_slash {
        url2.trim_end_matches('/')
    } else {
        url2
    };
    str_cmp_i_w(a, b)
}

pub fn url_create_from_path_w(path: &str, url: &mut String) -> i32 {
    url.clear();
    url.push_str("file:///");
    url.push_str(&path.replace('\\', "/"));
    S_OK
}

pub fn url_escape_w(url: &str, out: &mut String, _flags: u32) -> i32 {
    out.clear();
    for &b in url.as_bytes() {
        match b {
            b' ' => out.push_str("%20"),
            b'<' => out.push_str("%3C"),
            b'>' => out.push_str("%3E"),
            b'"' => out.push_str("%22"),
            b'#' => out.push_str("%23"),
            b'{' => out.push_str("%7B"),
            b'}' => out.push_str("%7D"),
            b'|' => out.push_str("%7C"),
            b'^' => out.push_str("%5E"),
            b'[' => out.push_str("%5B"),
            b']' => out.push_str("%5D"),
            b'`' => out.push_str("%60"),
            _ => out.push(b as char),
        }
    }
    S_OK
}

pub fn url_fixup_w(url: &str, out: &mut String) -> i32 {
    out.clear();
    if !url.contains("://") && url.contains('.') {
        out.push_str("http://");
    }
    out.push_str(url);
    S_OK
}

pub fn url_get_location_w(url: &str) -> String {
    if let Some(pos) = url.find('#') {
        String::from(&url[pos..])
    } else {
        String::new()
    }
}

pub fn url_get_part_w(url: &str, part: u32, out: &mut String) -> i32 {
    out.clear();
    match part {
        URL_PART_SCHEME => {
            if let Some(pos) = url.find("://") {
                out.push_str(&url[..pos]);
            }
        }
        URL_PART_HOSTNAME => {
            if let Some(scheme_end) = url.find("://") {
                let after = &url[scheme_end + 3..];
                let end = after
                    .find(|c: char| c == '/' || c == ':' || c == '?')
                    .unwrap_or(after.len());
                out.push_str(&after[..end]);
            }
        }
        URL_PART_PORT => {
            if let Some(scheme_end) = url.find("://") {
                let after = &url[scheme_end + 3..];
                if let Some(colon) = after.find(':') {
                    let rest = &after[colon + 1..];
                    let end = rest
                        .find(|c: char| c == '/' || c == '?')
                        .unwrap_or(rest.len());
                    out.push_str(&rest[..end]);
                }
            }
        }
        URL_PART_QUERY => {
            if let Some(pos) = url.find('?') {
                let end = url[pos..].find('#').unwrap_or(url.len() - pos);
                out.push_str(&url[pos + 1..pos + end]);
            }
        }
        _ => return E_INVALIDARG,
    }
    S_OK
}

pub fn url_hash_w(url: &str, hash: &mut [u8]) -> i32 {
    if hash.is_empty() {
        return E_INVALIDARG;
    }
    let mut h: u32 = 0x811c9dc5;
    for &b in url.as_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    let bytes = h.to_le_bytes();
    for (i, slot) in hash.iter_mut().enumerate() {
        *slot = bytes[i % 4];
    }
    S_OK
}

pub fn url_is_w(url: &str, kind: u32) -> bool {
    match kind {
        0 => url.contains("://"),     // URLIS_URL
        1 => url.find('?').is_some(), // URLIS_HASQUERY
        2 => !url.contains("://"),    // URLIS_NOHISTORY
        3 => !url.contains('/'),      // URLIS_OPAQUE
        _ => false,
    }
}

pub fn url_is_file_url_w(url: &str) -> bool {
    url.starts_with("file://")
}

pub fn url_is_no_history_w(url: &str) -> bool {
    url.starts_with("javascript:") || url.starts_with("about:")
}

pub fn url_is_opaque_w(url: &str) -> bool {
    url.starts_with("mailto:") || url.starts_with("javascript:")
}

pub fn url_unescape_w(url: &str, out: &mut String) -> i32 {
    out.clear();
    let bytes = url.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_val(bytes[i + 1]);
            let lo = hex_val(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    S_OK
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// =========================================================================
// Registry Utility Functions
// =========================================================================

pub fn sh_reg_get_value_w(
    ctx: &mut CompatContext,
    key_path: &str,
    value_name: &str,
) -> Option<String> {
    let val = ctx.registry.get_value(key_path, value_name);
    if let Some(val) = val {
        set_last_error(ctx, ERROR_SUCCESS);
        match &val {
            crate::RegValue::String(s) => Some(s.clone()),
            crate::RegValue::DWord(d) => Some(alloc::format!("{}", d)),
            crate::RegValue::QWord(q) => Some(alloc::format!("{}", q)),
            crate::RegValue::ExpandString(s) => Some(s.clone()),
            _ => Some(String::from("<binary>")),
        }
    } else {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        None
    }
}

pub fn sh_reg_set_value_w(
    ctx: &mut CompatContext,
    key_path: &str,
    value_name: &str,
    value: &str,
) -> u32 {
    ctx.registry.set_value(
        key_path,
        value_name,
        crate::RegValue::String(String::from(value)),
    );
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn sh_reg_delete_key_w(ctx: &mut CompatContext, _key_path: &str) -> u32 {
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn sh_reg_enum_key_ex_w(ctx: &mut CompatContext, key_path: &str, index: u32) -> Option<String> {
    let all_keys: Vec<String> = ctx.registry.enumerate_keys();
    let matching: Vec<&String> = all_keys
        .iter()
        .filter(|k| k.starts_with(key_path))
        .collect();

    if (index as usize) < matching.len() {
        let result = matching[index as usize].clone();
        set_last_error(ctx, ERROR_SUCCESS);
        Some(result)
    } else {
        set_last_error(ctx, 259); // ERROR_NO_MORE_ITEMS
        None
    }
}

pub fn sh_reg_enum_value_w(
    ctx: &mut CompatContext,
    key_path: &str,
    index: u32,
) -> Option<(String, String)> {
    let result = ctx.registry.enumerate_values(key_path).and_then(|v| {
        let (ref name, ref val) = *v.get(index as usize)?;
        let val_str = match val {
            crate::RegValue::String(s) => String::from(s.as_str()),
            crate::RegValue::DWord(d) => alloc::format!("{}", d),
            _ => String::from("<binary>"),
        };
        Some((name.clone(), val_str))
    });
    if result.is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        result
    } else {
        set_last_error(ctx, 259);
        None
    }
}

#[derive(Debug)]
pub struct USKey {
    pub hkcu_path: String,
    pub hklm_path: String,
}

pub fn sh_reg_create_us_key_w(key: &str, _access: u32) -> USKey {
    USKey {
        hkcu_path: alloc::format!("HKCU\\{}", key),
        hklm_path: alloc::format!("HKLM\\{}", key),
    }
}

pub fn sh_reg_open_us_key_w(key: &str, _access: u32) -> Option<USKey> {
    Some(USKey {
        hkcu_path: alloc::format!("HKCU\\{}", key),
        hklm_path: alloc::format!("HKLM\\{}", key),
    })
}

pub fn sh_reg_close_us_key(_key: USKey) -> u32 {
    ERROR_SUCCESS
}

pub fn sh_reg_query_us_value_w(
    ctx: &mut CompatContext,
    key: &USKey,
    value_name: &str,
) -> Option<String> {
    if let Some(v) = sh_reg_get_value_w(ctx, &key.hkcu_path, value_name) {
        return Some(v);
    }
    sh_reg_get_value_w(ctx, &key.hklm_path, value_name)
}

pub fn sh_reg_write_us_value_w(
    ctx: &mut CompatContext,
    key: &USKey,
    value_name: &str,
    value: &str,
    flags: u32,
) -> u32 {
    if flags & SHREGSET_HKCU != 0 || flags & SHREGSET_FORCE_HKCU != 0 {
        sh_reg_set_value_w(ctx, &key.hkcu_path, value_name, value);
    }
    if flags & SHREGSET_HKLM != 0 || flags & SHREGSET_FORCE_HKLM != 0 {
        sh_reg_set_value_w(ctx, &key.hklm_path, value_name, value);
    }
    ERROR_SUCCESS
}

// =========================================================================
// Other Utilities
// =========================================================================

pub fn sh_auto_complete(_hwnd: u64, _flags: u32) -> i32 {
    S_OK
}

pub fn sh_create_stream_on_file_w(_path: &str, _mode: u32) -> Option<u64> {
    Some(0xBEEF0001)
}

pub fn sh_create_thread(_func: u64, _data: u64) -> bool {
    true
}

pub fn sh_delete_key_w(ctx: &mut CompatContext, _key: u64, sub_key: &str) -> u32 {
    let _ = sub_key;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn sh_delete_value_w(ctx: &mut CompatContext, _key: u64, _sub_key: &str, value: &str) -> u32 {
    let _ = value;
    set_last_error(ctx, ERROR_SUCCESS);
    ERROR_SUCCESS
}

pub fn sh_get_value_w(ctx: &mut CompatContext, key_path: &str, value_name: &str) -> Option<String> {
    sh_reg_get_value_w(ctx, key_path, value_name)
}

pub fn sh_set_value_w(
    ctx: &mut CompatContext,
    key_path: &str,
    value_name: &str,
    value: &str,
) -> u32 {
    sh_reg_set_value_w(ctx, key_path, value_name, value)
}

pub fn assoc_query_string_w(
    _ctx: &mut CompatContext,
    _flags: u32,
    str_type: u32,
    assoc: &str,
    _extra: Option<&str>,
    result: &mut String,
) -> i32 {
    match str_type {
        ASSOCSTR_EXECUTABLE => {
            result.clear();
            result.push_str("C:\\Program Files\\");
            result.push_str(assoc);
            result.push_str("\\app.exe");
            S_OK
        }
        ASSOCSTR_FRIENDLYAPPNAME => {
            result.clear();
            result.push_str(assoc);
            result.push_str(" Application");
            S_OK
        }
        ASSOCSTR_CONTENTTYPE => {
            result.clear();
            result.push_str("application/octet-stream");
            S_OK
        }
        _ => E_INVALIDARG,
    }
}

pub fn color_adjust_luma(rgb: u32, n: i32, _scale: bool) -> u32 {
    let r = ((rgb >> 16) & 0xFF) as i32;
    let g = ((rgb >> 8) & 0xFF) as i32;
    let b = (rgb & 0xFF) as i32;
    let clamp = |v: i32| -> u32 { v.max(0).min(255) as u32 };
    (clamp(r + n) << 16) | (clamp(g + n) << 8) | clamp(b + n)
}

pub fn color_hls_to_rgb(hue: u16, luminance: u16, saturation: u16) -> u32 {
    if saturation == 0 {
        let l = (luminance as u32 * 255 / 240).min(255);
        return (l << 16) | (l << 8) | l;
    }
    let h = hue as f64 / 240.0;
    let l = luminance as f64 / 240.0;
    let s = saturation as f64 / 240.0;

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    let hue_to_rgb = |t: f64| -> u32 {
        let mut t = t;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let val = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (val * 255.0).min(255.0).max(0.0) as u32
    };

    let r = hue_to_rgb(h + 1.0 / 3.0);
    let g = hue_to_rgb(h);
    let b = hue_to_rgb(h - 1.0 / 3.0);
    (r << 16) | (g << 8) | b
}

pub fn color_rgb_to_hls(rgb: u32, hue: &mut u16, luminance: &mut u16, saturation: &mut u16) {
    let r = ((rgb >> 16) & 0xFF) as f64 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f64 / 255.0;
    let b = (rgb & 0xFF) as f64 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < 1e-10 {
        *hue = 0;
        *saturation = 0;
        *luminance = (l * 240.0) as u16;
        return;
    }

    let s = if l < 0.5 {
        (max - min) / (max + min)
    } else {
        (max - min) / (2.0 - max - min)
    };

    let h = if (r - max).abs() < 1e-10 {
        (g - b) / (max - min)
    } else if (g - max).abs() < 1e-10 {
        2.0 + (b - r) / (max - min)
    } else {
        4.0 + (r - g) / (max - min)
    };

    let h = if h < 0.0 { h + 6.0 } else { h };
    *hue = (h / 6.0 * 240.0) as u16;
    *luminance = (l * 240.0) as u16;
    *saturation = (s * 240.0) as u16;
}

pub fn hash_data(data: &[u8], hash: &mut [u8]) -> i32 {
    if hash.is_empty() {
        return E_INVALIDARG;
    }
    let mut h: u32 = 0x811c9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    let bytes = h.to_le_bytes();
    for (i, slot) in hash.iter_mut().enumerate() {
        *slot = bytes[i % 4];
    }
    S_OK
}

// =========================================================================
// Global SHLWAPI runtime
// =========================================================================

static SHLWAPI_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct ShlwapiRuntime {
    pub autocomplete_enabled: bool,
    pub url_cache: BTreeMap<String, String>,
}

impl ShlwapiRuntime {
    fn new() -> Self {
        Self {
            autocomplete_enabled: true,
            url_cache: BTreeMap::new(),
        }
    }
}

static mut SHLWAPI_INNER: Option<ShlwapiRuntime> = None;

pub fn init() {
    if SHLWAPI_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            SHLWAPI_INNER = Some(ShlwapiRuntime::new());
        }
    }
}

pub fn runtime() -> Option<&'static ShlwapiRuntime> {
    if SHLWAPI_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { SHLWAPI_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut ShlwapiRuntime> {
    if SHLWAPI_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { SHLWAPI_INNER.as_mut() }
    } else {
        None
    }
}
