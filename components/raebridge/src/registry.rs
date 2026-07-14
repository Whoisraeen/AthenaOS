//! Windows Registry Hive Emulator for RaeBridge.
//!
//! Provides a tree-based in-memory Windows registry with sensible Windows 10
//! defaults. Many Windows applications check registry keys for OS version,
//! processor count, system paths, and other configuration. This module
//! provides those values out of the box.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// =========================================================================
// Registry value types (mirrors the REG_* constants from advapi32)
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
// Predefined HKEY constants
// =========================================================================

pub const HKEY_CLASSES_ROOT: u64 = 0x80000000;
pub const HKEY_CURRENT_USER: u64 = 0x80000001;
pub const HKEY_LOCAL_MACHINE: u64 = 0x80000002;
pub const HKEY_USERS: u64 = 0x80000003;
pub const HKEY_PERFORMANCE_DATA: u64 = 0x80000004;
pub const HKEY_CURRENT_CONFIG: u64 = 0x80000005;

// =========================================================================
// Registry value
// =========================================================================

#[derive(Debug, Clone)]
pub enum RegistryValue {
    None,
    Sz(String),
    ExpandSz(String),
    Binary(Vec<u8>),
    DWord(u32),
    DWordBigEndian(u32),
    Link(String),
    MultiSz(Vec<String>),
    QWord(u64),
}

impl RegistryValue {
    pub fn reg_type(&self) -> u32 {
        match self {
            Self::None => REG_NONE,
            Self::Sz(_) => REG_SZ,
            Self::ExpandSz(_) => REG_EXPAND_SZ,
            Self::Binary(_) => REG_BINARY,
            Self::DWord(_) => REG_DWORD,
            Self::DWordBigEndian(_) => REG_DWORD_BIG_ENDIAN,
            Self::Link(_) => REG_LINK,
            Self::MultiSz(_) => REG_MULTI_SZ,
            Self::QWord(_) => REG_QWORD,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::None => Vec::new(),
            Self::Sz(s) => string_to_wide_bytes(s),
            Self::ExpandSz(s) => string_to_wide_bytes(s),
            Self::Binary(b) => b.clone(),
            Self::DWord(v) => v.to_le_bytes().to_vec(),
            Self::DWordBigEndian(v) => v.to_be_bytes().to_vec(),
            Self::Link(s) => string_to_wide_bytes(s),
            Self::MultiSz(ss) => {
                let joined = ss.join("\0");
                let mut bytes = string_to_wide_bytes(&joined);
                bytes.extend_from_slice(&[0, 0]);
                bytes
            }
            Self::QWord(v) => v.to_le_bytes().to_vec(),
        }
    }

    pub fn byte_len(&self) -> usize {
        self.to_bytes().len()
    }
}

fn string_to_wide_bytes(s: &str) -> Vec<u8> {
    let wide: Vec<u16> = s.encode_utf16().chain(core::iter::once(0)).collect();
    wide.iter().flat_map(|w| w.to_le_bytes()).collect()
}

// =========================================================================
// Registry key node
// =========================================================================

#[derive(Debug, Clone)]
pub struct RegistryKey {
    pub name: String,
    pub values: BTreeMap<String, RegistryValue>,
    pub subkeys: BTreeMap<String, RegistryKey>,
    pub class_name: Option<String>,
    pub last_write_time: u64,
}

impl RegistryKey {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            values: BTreeMap::new(),
            subkeys: BTreeMap::new(),
            class_name: None,
            last_write_time: 133_500_000_000_000_000,
        }
    }

    // The Windows registry is case-INSENSITIVE for key and value names
    // (case-preserving on create). Every accessor takes the exact-match
    // fast path first, then falls back to an ASCII-case-insensitive scan —
    // apps freely mix "SOFTWARE"/"Software" and must hit the same key.

    /// The stored name that matches `name` case-insensitively, if any.
    fn canonical_value_name(&self, name: &str) -> Option<String> {
        if self.values.contains_key(name) {
            return Some(String::from(name));
        }
        self.values
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
    }

    fn canonical_subkey_name(&self, name: &str) -> Option<String> {
        if self.subkeys.contains_key(name) {
            return Some(String::from(name));
        }
        self.subkeys
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
    }

    pub fn set_value(&mut self, name: &str, value: RegistryValue) {
        let key = self
            .canonical_value_name(name)
            .unwrap_or_else(|| String::from(name));
        self.values.insert(key, value);
        self.last_write_time += 1;
    }

    pub fn get_value(&self, name: &str) -> Option<&RegistryValue> {
        if let Some(v) = self.values.get(name) {
            return Some(v);
        }
        self.values
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn delete_value(&mut self, name: &str) -> bool {
        match self.canonical_value_name(name) {
            Some(k) => self.values.remove(&k).is_some(),
            None => false,
        }
    }

    pub fn create_subkey(&mut self, name: &str) -> &mut RegistryKey {
        let key = self
            .canonical_subkey_name(name)
            .unwrap_or_else(|| String::from(name));
        self.subkeys
            .entry(key)
            .or_insert_with(|| RegistryKey::new(name))
    }

    pub fn get_subkey(&self, name: &str) -> Option<&RegistryKey> {
        if let Some(k) = self.subkeys.get(name) {
            return Some(k);
        }
        self.subkeys
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn get_subkey_mut(&mut self, name: &str) -> Option<&mut RegistryKey> {
        let key = self.canonical_subkey_name(name)?;
        self.subkeys.get_mut(&key)
    }

    pub fn delete_subkey(&mut self, name: &str) -> bool {
        match self.canonical_subkey_name(name) {
            Some(k) => self.subkeys.remove(&k).is_some(),
            None => false,
        }
    }

    pub fn subkey_count(&self) -> usize {
        self.subkeys.len()
    }

    pub fn value_count(&self) -> usize {
        self.values.len()
    }

    pub fn max_subkey_name_len(&self) -> usize {
        self.subkeys.keys().map(|k| k.len()).max().unwrap_or(0)
    }

    pub fn max_value_name_len(&self) -> usize {
        self.values.keys().map(|k| k.len()).max().unwrap_or(0)
    }

    pub fn max_value_data_len(&self) -> usize {
        self.values
            .values()
            .map(|v| v.byte_len())
            .max()
            .unwrap_or(0)
    }

    pub fn subkey_names(&self) -> Vec<&String> {
        self.subkeys.keys().collect()
    }

    pub fn value_entries(&self) -> Vec<(&String, &RegistryValue)> {
        self.values.iter().collect()
    }
}

// =========================================================================
// Registry hive — manages root keys and handle mapping
// =========================================================================

pub struct RegistryHive {
    pub hkcr: RegistryKey,
    pub hkcu: RegistryKey,
    pub hklm: RegistryKey,
    pub hku: RegistryKey,
    pub hkcc: RegistryKey,
    pub hkpd: RegistryKey,
    handles: BTreeMap<u64, String>,
    next_handle: u64,
}

impl RegistryHive {
    pub fn new() -> Self {
        let mut hive = Self {
            hkcr: RegistryKey::new("HKCR"),
            hkcu: RegistryKey::new("HKCU"),
            hklm: RegistryKey::new("HKLM"),
            hku: RegistryKey::new("HKU"),
            hkcc: RegistryKey::new("HKCC"),
            hkpd: RegistryKey::new("HKPD"),
            handles: BTreeMap::new(),
            next_handle: 0x100,
        };
        hive.populate_defaults();
        hive
    }

    // -----------------------------------------------------------------
    // Default population — Windows 10 22H2 compatible values
    // -----------------------------------------------------------------

    fn populate_defaults(&mut self) {
        self.populate_nt_current_version();
        self.populate_system_control();
        self.populate_hkcu_defaults();
        self.populate_hkcr_defaults();
        self.populate_hkcc_defaults();
        self.populate_environment();
        self.populate_hardware_info();
        self.populate_steam_and_gaming();
        self.populate_directx();
    }

    fn populate_nt_current_version(&mut self) {
        let cv = self.ensure_key_mut("HKLM", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        cv.set_value(
            "ProductName",
            RegistryValue::Sz(String::from("Windows 10 Pro")),
        );
        cv.set_value("EditionID", RegistryValue::Sz(String::from("Professional")));
        cv.set_value("CurrentBuild", RegistryValue::Sz(String::from("22631")));
        cv.set_value(
            "CurrentBuildNumber",
            RegistryValue::Sz(String::from("22631")),
        );
        cv.set_value("CurrentVersion", RegistryValue::Sz(String::from("6.3")));
        cv.set_value("CurrentMajorVersionNumber", RegistryValue::DWord(10));
        cv.set_value("CurrentMinorVersionNumber", RegistryValue::DWord(0));
        cv.set_value("UBR", RegistryValue::DWord(4602));
        cv.set_value("DisplayVersion", RegistryValue::Sz(String::from("23H2")));
        cv.set_value("ReleaseId", RegistryValue::Sz(String::from("2009")));
        cv.set_value("BuildBranch", RegistryValue::Sz(String::from("ni_release")));
        cv.set_value(
            "BuildLab",
            RegistryValue::Sz(String::from("22631.ni_release.231005-1735")),
        );
        cv.set_value(
            "CompositionEditionID",
            RegistryValue::Sz(String::from("Enterprise")),
        );
        cv.set_value(
            "RegisteredOrganization",
            RegistryValue::Sz(String::from("RaeenOS")),
        );
        cv.set_value("RegisteredOwner", RegistryValue::Sz(String::from("user")));
        cv.set_value(
            "InstallationType",
            RegistryValue::Sz(String::from("Client")),
        );
        cv.set_value("SystemRoot", RegistryValue::Sz(String::from(r"C:\Windows")));
        cv.set_value("PathName", RegistryValue::Sz(String::from(r"C:\Windows")));
        cv.set_value(
            "ProductId",
            RegistryValue::Sz(String::from("00330-80000-00000-AA001")),
        );
        cv.set_value("InstallDate", RegistryValue::DWord(1700000000));
    }

    fn populate_system_control(&mut self) {
        let ctrl = self.ensure_key_mut("HKLM", r"SYSTEM\CurrentControlSet\Control");
        ctrl.set_value("CurrentUser", RegistryValue::Sz(String::from("USERNAME")));
        ctrl.set_value(
            "WaitToKillServiceTimeout",
            RegistryValue::Sz(String::from("5000")),
        );

        let sess = self.ensure_key_mut("HKLM", r"SYSTEM\CurrentControlSet\Control\Session Manager");
        sess.set_value(
            "PendingFileRenameOperations",
            RegistryValue::MultiSz(Vec::new()),
        );

        let mm = self.ensure_key_mut(
            "HKLM",
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management",
        );
        mm.set_value(
            "PagingFiles",
            RegistryValue::MultiSz(alloc::vec![String::from(r"C:\pagefile.sys 4096 8192")]),
        );

        let nls = self.ensure_key_mut("HKLM", r"SYSTEM\CurrentControlSet\Control\Nls\CodePage");
        nls.set_value("ACP", RegistryValue::Sz(String::from("1252")));
        nls.set_value("OEMCP", RegistryValue::Sz(String::from("437")));
        nls.set_value("MACCP", RegistryValue::Sz(String::from("10000")));

        let csd = self.ensure_key_mut(
            "HKLM",
            r"SYSTEM\CurrentControlSet\Control\ComputerName\ActiveComputerName",
        );
        csd.set_value("ComputerName", RegistryValue::Sz(String::from("RAEENOS")));

        let tz = self.ensure_key_mut(
            "HKLM",
            r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation",
        );
        tz.set_value("TimeZoneKeyName", RegistryValue::Sz(String::from("UTC")));
        tz.set_value("ActiveTimeBias", RegistryValue::DWord(0));

        let fs = self.ensure_key_mut("HKLM", r"SYSTEM\CurrentControlSet\Control\FileSystem");
        fs.set_value("LongPathsEnabled", RegistryValue::DWord(1));
        fs.set_value("NtfsDisable8dot3NameCreation", RegistryValue::DWord(1));
    }

    fn populate_environment(&mut self) {
        let env = self.ensure_key_mut(
            "HKLM",
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
        );
        env.set_value("OS", RegistryValue::Sz(String::from("Windows_NT")));
        env.set_value(
            "PROCESSOR_ARCHITECTURE",
            RegistryValue::Sz(String::from("AMD64")),
        );
        env.set_value("NUMBER_OF_PROCESSORS", RegistryValue::Sz(String::from("8")));
        env.set_value(
            "PROCESSOR_IDENTIFIER",
            RegistryValue::Sz(String::from(
                "AMD64 Family 25 Model 33 Stepping 2, AuthenticAMD",
            )),
        );
        env.set_value("PROCESSOR_LEVEL", RegistryValue::Sz(String::from("25")));
        env.set_value(
            "PROCESSOR_REVISION",
            RegistryValue::Sz(String::from("2102")),
        );
        env.set_value(
            "ComSpec",
            RegistryValue::ExpandSz(String::from(r"%SystemRoot%\system32\cmd.exe")),
        );
        env.set_value(
            "Path",
            RegistryValue::ExpandSz(String::from(
                r"%SystemRoot%\system32;%SystemRoot%;%SystemRoot%\System32\Wbem",
            )),
        );
        env.set_value(
            "PATHEXT",
            RegistryValue::Sz(String::from(
                ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC",
            )),
        );
        env.set_value(
            "TEMP",
            RegistryValue::ExpandSz(String::from(r"%SystemRoot%\TEMP")),
        );
        env.set_value(
            "TMP",
            RegistryValue::ExpandSz(String::from(r"%SystemRoot%\TEMP")),
        );
        env.set_value("windir", RegistryValue::Sz(String::from(r"C:\Windows")));
    }

    fn populate_hardware_info(&mut self) {
        let cpu0 = self.ensure_key_mut("HKLM", r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
        cpu0.set_value(
            "ProcessorNameString",
            RegistryValue::Sz(String::from("RaeenOS Virtual Processor")),
        );
        cpu0.set_value(
            "Identifier",
            RegistryValue::Sz(String::from("x86 Family 25 Model 33 Stepping 2")),
        );
        cpu0.set_value(
            "VendorIdentifier",
            RegistryValue::Sz(String::from("AuthenticAMD")),
        );
        cpu0.set_value("~MHz", RegistryValue::DWord(3600));
        cpu0.set_value("FeatureSet", RegistryValue::DWord(0x7FFAFBFF));

        let bios = self.ensure_key_mut("HKLM", r"HARDWARE\DESCRIPTION\System\BIOS");
        bios.set_value(
            "SystemManufacturer",
            RegistryValue::Sz(String::from("RaeenOS")),
        );
        bios.set_value(
            "SystemProductName",
            RegistryValue::Sz(String::from("RaeStation")),
        );
        bios.set_value(
            "BIOSVendor",
            RegistryValue::Sz(String::from("RaeenOS BIOS")),
        );
        bios.set_value("BIOSVersion", RegistryValue::Sz(String::from("1.0.0")));

        let sys = self.ensure_key_mut("HKLM", r"HARDWARE\DESCRIPTION\System");
        sys.set_value(
            "Identifier",
            RegistryValue::Sz(String::from("AT/AT COMPATIBLE")),
        );
        sys.set_value(
            "SystemBiosVersion",
            RegistryValue::MultiSz(alloc::vec![
                String::from("RAEENOS - 1"),
                String::from("1.0.0")
            ]),
        );
    }

    fn populate_hkcu_defaults(&mut self) {
        let _sw = self.ensure_key_mut("HKCU", "Software");

        let env = self.ensure_key_mut("HKCU", r"Environment");
        env.set_value(
            "TEMP",
            RegistryValue::ExpandSz(String::from(r"%USERPROFILE%\AppData\Local\Temp")),
        );
        env.set_value(
            "TMP",
            RegistryValue::ExpandSz(String::from(r"%USERPROFILE%\AppData\Local\Temp")),
        );

        let console = self.ensure_key_mut("HKCU", "Console");
        console.set_value("ScreenBufferSize", RegistryValue::DWord(0x0019_0050));
        console.set_value("WindowSize", RegistryValue::DWord(0x0019_0050));
        console.set_value("FontSize", RegistryValue::DWord(0x0012_0000));
        console.set_value("QuickEdit", RegistryValue::DWord(1));

        let vol = self.ensure_key_mut(
            "HKCU",
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
        );
        vol.set_value("HideFileExt", RegistryValue::DWord(1));
        vol.set_value("ShowSuperHidden", RegistryValue::DWord(0));

        let intl = self.ensure_key_mut("HKCU", r"Control Panel\International");
        intl.set_value("Locale", RegistryValue::Sz(String::from("00000409")));
        intl.set_value("LocaleName", RegistryValue::Sz(String::from("en-US")));
        intl.set_value("sLanguage", RegistryValue::Sz(String::from("ENU")));
        intl.set_value("sCountry", RegistryValue::Sz(String::from("United States")));
    }

    fn populate_hkcr_defaults(&mut self) {
        let exe = self.ensure_key_mut("HKCR", ".exe");
        exe.set_value("", RegistryValue::Sz(String::from("exefile")));
        exe.set_value(
            "Content Type",
            RegistryValue::Sz(String::from("application/x-msdownload")),
        );

        let dll = self.ensure_key_mut("HKCR", ".dll");
        dll.set_value("", RegistryValue::Sz(String::from("dllfile")));
        dll.set_value(
            "Content Type",
            RegistryValue::Sz(String::from("application/x-msdownload")),
        );

        let txt = self.ensure_key_mut("HKCR", ".txt");
        txt.set_value("", RegistryValue::Sz(String::from("txtfile")));
        txt.set_value(
            "Content Type",
            RegistryValue::Sz(String::from("text/plain")),
        );
    }

    fn populate_hkcc_defaults(&mut self) {
        let disp = self.ensure_key_mut("HKCC", r"Software\Fonts");
        disp.set_value("LogPixels", RegistryValue::DWord(96));
    }

    fn populate_steam_and_gaming(&mut self) {
        let steam = self.ensure_key_mut("HKLM", r"SOFTWARE\Valve\Steam");
        steam.set_value(
            "InstallPath",
            RegistryValue::Sz(String::from(r"C:\Program Files (x86)\Steam")),
        );
        steam.set_value(
            "SteamExe",
            RegistryValue::Sz(String::from(r"C:\Program Files (x86)\Steam\steam.exe")),
        );
        steam.set_value(
            "SteamPath",
            RegistryValue::Sz(String::from(r"C:/Program Files (x86)/Steam")),
        );
        steam.set_value("Language", RegistryValue::Sz(String::from("english")));
        steam.set_value("Universe", RegistryValue::Sz(String::from("Public")));

        let steam32 = self.ensure_key_mut("HKLM", r"SOFTWARE\WOW6432Node\Valve\Steam");
        steam32.set_value(
            "InstallPath",
            RegistryValue::Sz(String::from(r"C:\Program Files (x86)\Steam")),
        );
        steam32.set_value(
            "SteamExe",
            RegistryValue::Sz(String::from(r"C:\Program Files (x86)\Steam\steam.exe")),
        );

        let su = self.ensure_key_mut("HKCU", r"Software\Valve\Steam");
        su.set_value(
            "SteamPath",
            RegistryValue::Sz(String::from(r"C:/Program Files (x86)/Steam")),
        );
        su.set_value(
            "SteamExe",
            RegistryValue::Sz(String::from(r"C:\Program Files (x86)\Steam\steam.exe")),
        );
        su.set_value("Language", RegistryValue::Sz(String::from("english")));
        su.set_value("AlreadyRetriedOfflineMode", RegistryValue::DWord(0));
        su.set_value("RememberPassword", RegistryValue::DWord(1));
        su.set_value("RunningAppID", RegistryValue::DWord(0));

        let apps = self.ensure_key_mut("HKCU", r"Software\Valve\Steam\Apps");
        let _ = apps;

        let xi = self.ensure_key_mut("HKLM", r"SOFTWARE\Microsoft\XInput");
        xi.set_value("Version", RegistryValue::Sz(String::from("1.4")));
    }

    fn populate_directx(&mut self) {
        let dx = self.ensure_key_mut("HKLM", r"SOFTWARE\Microsoft\DirectX");
        dx.set_value("Version", RegistryValue::Sz(String::from("4.09.00.0904")));
        dx.set_value("InstalledVersion", RegistryValue::DWord(0x0004_0009));

        let dxs = self.ensure_key_mut("HKLM", r"SOFTWARE\Microsoft\Direct3D\Drivers");
        dxs.set_value("SoftwareOnly", RegistryValue::DWord(0));

        let dxd = self.ensure_key_mut("HKLM", r"SOFTWARE\Microsoft\DirectDraw");
        dxd.set_value("EmulationOnly", RegistryValue::DWord(0));

        let vc = self.ensure_key_mut(
            "HKLM",
            r"SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\X64",
        );
        vc.set_value("Installed", RegistryValue::DWord(1));
        vc.set_value("Major", RegistryValue::DWord(14));
        vc.set_value("Minor", RegistryValue::DWord(38));
        vc.set_value("Bld", RegistryValue::DWord(33135));

        let dotnet = self.ensure_key_mut(
            "HKLM",
            r"SOFTWARE\Microsoft\NET Framework Setup\NDP\v4\Full",
        );
        dotnet.set_value("Release", RegistryValue::DWord(528049));
        dotnet.set_value("Version", RegistryValue::Sz(String::from("4.8.04084")));
        dotnet.set_value("Install", RegistryValue::DWord(1));
    }

    // -----------------------------------------------------------------
    // Root key lookup
    // -----------------------------------------------------------------

    fn root_key(&self, prefix: &str) -> Option<&RegistryKey> {
        match prefix {
            "HKCR" | "HKEY_CLASSES_ROOT" => Some(&self.hkcr),
            "HKCU" | "HKEY_CURRENT_USER" => Some(&self.hkcu),
            "HKLM" | "HKEY_LOCAL_MACHINE" => Some(&self.hklm),
            "HKU" | "HKEY_USERS" => Some(&self.hku),
            "HKCC" | "HKEY_CURRENT_CONFIG" => Some(&self.hkcc),
            "HKPD" | "HKEY_PERFORMANCE_DATA" => Some(&self.hkpd),
            _ => None,
        }
    }

    fn root_key_mut(&mut self, prefix: &str) -> Option<&mut RegistryKey> {
        match prefix {
            "HKCR" | "HKEY_CLASSES_ROOT" => Some(&mut self.hkcr),
            "HKCU" | "HKEY_CURRENT_USER" => Some(&mut self.hkcu),
            "HKLM" | "HKEY_LOCAL_MACHINE" => Some(&mut self.hklm),
            "HKU" | "HKEY_USERS" => Some(&mut self.hku),
            "HKCC" | "HKEY_CURRENT_CONFIG" => Some(&mut self.hkcc),
            "HKPD" | "HKEY_PERFORMANCE_DATA" => Some(&mut self.hkpd),
            _ => None,
        }
    }

    pub fn root_from_hkey(&self, hkey: u64) -> Option<&RegistryKey> {
        match hkey {
            HKEY_CLASSES_ROOT => Some(&self.hkcr),
            HKEY_CURRENT_USER => Some(&self.hkcu),
            HKEY_LOCAL_MACHINE => Some(&self.hklm),
            HKEY_USERS => Some(&self.hku),
            HKEY_CURRENT_CONFIG => Some(&self.hkcc),
            HKEY_PERFORMANCE_DATA => Some(&self.hkpd),
            _ => None,
        }
    }

    pub fn root_from_hkey_mut(&mut self, hkey: u64) -> Option<&mut RegistryKey> {
        match hkey {
            HKEY_CLASSES_ROOT => Some(&mut self.hkcr),
            HKEY_CURRENT_USER => Some(&mut self.hkcu),
            HKEY_LOCAL_MACHINE => Some(&mut self.hklm),
            HKEY_USERS => Some(&mut self.hku),
            HKEY_CURRENT_CONFIG => Some(&mut self.hkcc),
            HKEY_PERFORMANCE_DATA => Some(&mut self.hkpd),
            _ => None,
        }
    }

    pub fn hkey_prefix(hkey: u64) -> Option<&'static str> {
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

    pub fn is_predefined_hkey(hkey: u64) -> bool {
        hkey >= 0x80000000 && hkey <= 0x80000005
    }

    // -----------------------------------------------------------------
    // Path navigation through the tree
    // -----------------------------------------------------------------

    fn split_path(full_path: &str) -> (&str, &str) {
        if let Some(pos) = full_path.find('\\') {
            (&full_path[..pos], &full_path[pos + 1..])
        } else {
            (full_path, "")
        }
    }

    fn navigate_to<'a>(root: &'a RegistryKey, sub_path: &str) -> Option<&'a RegistryKey> {
        if sub_path.is_empty() {
            return Some(root);
        }
        let mut current = root;
        for component in sub_path.split('\\') {
            if component.is_empty() {
                continue;
            }
            match current.get_subkey(component) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    fn navigate_to_mut<'a>(
        root: &'a mut RegistryKey,
        sub_path: &str,
    ) -> Option<&'a mut RegistryKey> {
        if sub_path.is_empty() {
            return Some(root);
        }
        let mut current = root;
        for component in sub_path.split('\\') {
            if component.is_empty() {
                continue;
            }
            match current.get_subkey_mut(component) {
                Some(child) => current = child,
                None => return None,
            }
        }
        Some(current)
    }

    fn ensure_path_mut<'a>(root: &'a mut RegistryKey, sub_path: &str) -> &'a mut RegistryKey {
        if sub_path.is_empty() {
            return root;
        }
        let mut current = root;
        for component in sub_path.split('\\') {
            if component.is_empty() {
                continue;
            }
            current = current.create_subkey(component);
        }
        current
    }

    fn ensure_key_mut(&mut self, root_name: &str, sub_path: &str) -> &mut RegistryKey {
        let root = self.root_key_mut(root_name).expect("invalid root name");
        Self::ensure_path_mut(root, sub_path)
    }

    // -----------------------------------------------------------------
    // Handle management — maps HKEY handles to paths in the tree
    // -----------------------------------------------------------------

    pub fn alloc_handle(&mut self, path: String) -> u64 {
        let h = self.next_handle;
        self.next_handle += 4;
        self.handles.insert(h, path);
        h
    }

    pub fn close_handle(&mut self, handle: u64) -> bool {
        self.handles.remove(&handle).is_some()
    }

    pub fn handle_path(&self, handle: u64) -> Option<&String> {
        self.handles.get(&handle)
    }

    // -----------------------------------------------------------------
    // Public API — open / create / query / set / delete / enumerate
    // -----------------------------------------------------------------

    pub fn open_key(&mut self, hkey: u64, sub_key: &str) -> Result<u64, u32> {
        let full_path = self.resolve_path(hkey, sub_key)?;

        let (root_name, rest) = Self::split_path(&full_path);
        let root = self.root_key(root_name).ok_or(6u32)?; // ERROR_INVALID_HANDLE
        if Self::navigate_to(root, rest).is_none() {
            return Err(2); // ERROR_FILE_NOT_FOUND
        }

        Ok(self.alloc_handle(full_path))
    }

    pub fn create_key(&mut self, hkey: u64, sub_key: &str) -> Result<(u64, bool), u32> {
        let full_path = self.resolve_path(hkey, sub_key)?;
        let (root_name, rest) = Self::split_path(&full_path);

        let existed = {
            let root = self.root_key(root_name).ok_or(6u32)?;
            Self::navigate_to(root, rest).is_some()
        };

        {
            let root = self.root_key_mut(root_name).ok_or(6u32)?;
            Self::ensure_path_mut(root, rest);
        }

        let handle = self.alloc_handle(full_path);
        Ok((handle, existed))
    }

    pub fn query_value(&self, hkey: u64, value_name: &str) -> Result<&RegistryValue, u32> {
        let key = self.key_from_handle(hkey)?;
        key.get_value(value_name).ok_or(2) // ERROR_FILE_NOT_FOUND
    }

    pub fn set_value(
        &mut self,
        hkey: u64,
        value_name: &str,
        value: RegistryValue,
    ) -> Result<(), u32> {
        let key = self.key_from_handle_mut(hkey)?;
        key.set_value(value_name, value);
        Ok(())
    }

    pub fn delete_value(&mut self, hkey: u64, value_name: &str) -> Result<(), u32> {
        let key = self.key_from_handle_mut(hkey)?;
        if key.delete_value(value_name) {
            Ok(())
        } else {
            Err(2) // ERROR_FILE_NOT_FOUND
        }
    }

    pub fn delete_key(&mut self, hkey: u64, sub_key: &str) -> Result<(), u32> {
        let full_path = self.resolve_path(hkey, sub_key)?;
        let (root_name, rest) = Self::split_path(&full_path);

        if rest.is_empty() {
            return Err(5); // ERROR_ACCESS_DENIED — can't delete root keys
        }

        let (parent_path, child_name) = if let Some(pos) = rest.rfind('\\') {
            (&rest[..pos], &rest[pos + 1..])
        } else {
            ("", rest)
        };

        let root = self.root_key_mut(root_name).ok_or(6u32)?;
        let parent = Self::navigate_to_mut(root, parent_path).ok_or(2u32)?;

        if parent.delete_subkey(child_name) {
            Ok(())
        } else {
            Err(2) // ERROR_FILE_NOT_FOUND
        }
    }

    pub fn enum_subkeys(&self, hkey: u64, index: u32) -> Result<&String, u32> {
        let key = self.key_from_handle(hkey)?;
        let names = key.subkey_names();
        if (index as usize) >= names.len() {
            Err(259) // ERROR_NO_MORE_ITEMS
        } else {
            Ok(names[index as usize])
        }
    }

    pub fn enum_values(&self, hkey: u64, index: u32) -> Result<(&String, &RegistryValue), u32> {
        let key = self.key_from_handle(hkey)?;
        let entries = key.value_entries();
        if (index as usize) >= entries.len() {
            Err(259) // ERROR_NO_MORE_ITEMS
        } else {
            let (name, val) = entries[index as usize];
            Ok((name, val))
        }
    }

    pub fn query_info(&self, hkey: u64) -> Result<KeyInfo, u32> {
        let key = self.key_from_handle(hkey)?;
        Ok(KeyInfo {
            subkey_count: key.subkey_count() as u32,
            max_subkey_len: key.max_subkey_name_len() as u32,
            value_count: key.value_count() as u32,
            max_value_name_len: key.max_value_name_len() as u32,
            max_value_data_len: key.max_value_data_len() as u32,
            last_write_time: key.last_write_time,
            class_name: key.class_name.clone(),
        })
    }

    // -----------------------------------------------------------------
    // Flat-path compatibility API (used by existing lib.rs RegistryHive)
    // -----------------------------------------------------------------

    pub fn set_value_by_path(&mut self, key_path: &str, name: &str, value: RegistryValue) {
        let (root_name, rest) = Self::split_path(key_path);
        if let Some(root) = self.root_key_mut(root_name) {
            let key = Self::ensure_path_mut(root, rest);
            key.set_value(name, value);
        }
    }

    pub fn get_value_by_path(&self, key_path: &str, name: &str) -> Option<&RegistryValue> {
        let (root_name, rest) = Self::split_path(key_path);
        let root = self.root_key(root_name)?;
        let key = Self::navigate_to(root, rest)?;
        key.get_value(name)
    }

    pub fn key_exists_by_path(&self, key_path: &str) -> bool {
        let (root_name, rest) = Self::split_path(key_path);
        match self.root_key(root_name) {
            Some(root) => Self::navigate_to(root, rest).is_some(),
            None => false,
        }
    }

    pub fn delete_value_by_path(&mut self, key_path: &str, name: &str) -> bool {
        let (root_name, rest) = Self::split_path(key_path);
        if let Some(root) = self.root_key_mut(root_name) {
            if let Some(key) = Self::navigate_to_mut(root, rest) {
                return key.delete_value(name);
            }
        }
        false
    }

    pub fn enumerate_values_by_path(
        &self,
        key_path: &str,
    ) -> Option<Vec<(&String, &RegistryValue)>> {
        let (root_name, rest) = Self::split_path(key_path);
        let root = self.root_key(root_name)?;
        let key = Self::navigate_to(root, rest)?;
        Some(key.value_entries())
    }

    pub fn enumerate_subkeys_by_path(&self, key_path: &str) -> Option<Vec<&String>> {
        let (root_name, rest) = Self::split_path(key_path);
        let root = self.root_key(root_name)?;
        let key = Self::navigate_to(root, rest)?;
        Some(key.subkey_names())
    }

    // -----------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------

    fn resolve_path(&self, hkey: u64, sub_key: &str) -> Result<String, u32> {
        if let Some(prefix) = Self::hkey_prefix(hkey) {
            let mut path = String::from(prefix);
            if !sub_key.is_empty() {
                path.push('\\');
                path.push_str(sub_key);
            }
            return Ok(path);
        }

        if let Some(base_path) = self.handles.get(&hkey) {
            let mut path = base_path.clone();
            if !sub_key.is_empty() {
                path.push('\\');
                path.push_str(sub_key);
            }
            return Ok(path);
        }

        Err(6) // ERROR_INVALID_HANDLE
    }

    fn key_from_handle(&self, hkey: u64) -> Result<&RegistryKey, u32> {
        if let Some(root) = self.root_from_hkey(hkey) {
            return Ok(root);
        }

        let path = self.handles.get(&hkey).ok_or(6u32)?;
        let (root_name, rest) = Self::split_path(path);
        let root = self.root_key(root_name).ok_or(6u32)?;
        Self::navigate_to(root, rest).ok_or(2u32)
    }

    fn key_from_handle_mut(&mut self, hkey: u64) -> Result<&mut RegistryKey, u32> {
        if Self::is_predefined_hkey(hkey) {
            return self.root_from_hkey_mut(hkey).ok_or(6u32);
        }

        let path = self.handles.get(&hkey).ok_or(6u32)?.clone();
        let (root_name, rest) = Self::split_path(&path);
        let root = self.root_key_mut(root_name).ok_or(6u32)?;
        Self::navigate_to_mut(root, rest).ok_or(2u32)
    }
}

// =========================================================================
// Key information result
// =========================================================================

#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub subkey_count: u32,
    pub max_subkey_len: u32,
    pub value_count: u32,
    pub max_value_name_len: u32,
    pub max_value_data_len: u32,
    pub last_write_time: u64,
    pub class_name: Option<String>,
}
