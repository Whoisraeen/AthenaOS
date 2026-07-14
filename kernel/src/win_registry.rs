//! Win32 registry shim backed by versioned config (Concept §Compatibility:
//! "Windows apps see the registry they expect; AthenaOS stores it in ITS
//! config system — versioned, snapshottable, one-click-rollbackable like
//! every other setting"). MasterChecklist Phase 11.2 — "Registry shim
//! backed by versioned config".
//!
//! The hive emulator (`raebridge::registry::RegistryHive` — handles,
//! HKLM/HKCU roots, Windows-10 default keys) existed in-memory only: every
//! reboot lost whatever a Windows app wrote. This module owns the live
//! hive and BACKS it with `config_registry`: every `set_value` mirrors
//! into a versioned `/bridge/registry/...` key (journaled, snapshottable),
//! and boot replays those keys into the fresh hive — so registry writes
//! survive reboots and ride the same rollback story as native settings.
//!
//! The smoketest proves the full cycle: create/set/query through the shim,
//! the mirrored versioned-config key exists, and a COLD hive (drop +
//! re-init) resurrects the value purely from config. Deterministic on
//! QEMU and iron.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use raebridge::registry::{RegistryHive, RegistryValue};

pub use raebridge::registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

static HIVE: Mutex<Option<RegistryHive>> = Mutex::new(None);
static SETS_PERSISTED: AtomicU64 = AtomicU64::new(0);
static RESTORED_AT_BOOT: AtomicU64 = AtomicU64::new(0);

const CFG_PREFIX: &str = "/bridge/registry/";

/// `HKEY_CURRENT_USER\Software\Foo` + `Bar` → `/bridge/registry/HKEY_CURRENT_USER/Software/Foo/Bar`
fn cfg_key(reg_path: &str, value_name: &str) -> String {
    let mut k = String::from(CFG_PREFIX);
    for part in reg_path.split('\\').filter(|p| !p.is_empty()) {
        k.push_str(part);
        k.push('/');
    }
    k.push_str(value_name);
    k
}

/// Persisted text encoding: `<reg_type>:<payload>`.
fn serialize_value(v: &RegistryValue) -> Option<String> {
    Some(match v {
        RegistryValue::Sz(s) => alloc::format!("1:{}", s),
        RegistryValue::ExpandSz(s) => alloc::format!("2:{}", s),
        RegistryValue::Binary(b) => {
            let mut out = String::from("3:");
            for byte in b {
                out.push_str(&alloc::format!("{:02x}", byte));
            }
            out
        }
        RegistryValue::DWord(n) => alloc::format!("4:{}", n),
        RegistryValue::MultiSz(ss) => alloc::format!("7:{}", ss.join("\x1f")),
        RegistryValue::QWord(n) => alloc::format!("11:{}", n),
        _ => return None, // None/Link/BigEndian: not persisted
    })
}

fn parse_value(text: &str) -> Option<RegistryValue> {
    let (tag, payload) = text.split_once(':')?;
    Some(match tag {
        "1" => RegistryValue::Sz(String::from(payload)),
        "2" => RegistryValue::ExpandSz(String::from(payload)),
        "3" => {
            let bytes = payload.as_bytes();
            if bytes.len() % 2 != 0 {
                return None;
            }
            let mut out = Vec::with_capacity(bytes.len() / 2);
            for pair in bytes.chunks(2) {
                let hex = core::str::from_utf8(pair).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
            }
            RegistryValue::Binary(out)
        }
        "4" => RegistryValue::DWord(payload.parse().ok()?),
        "7" => RegistryValue::MultiSz(payload.split('\x1f').map(String::from).collect()),
        "11" => RegistryValue::QWord(payload.parse().ok()?),
        _ => return None,
    })
}

// ── Shim API (what the advapi32 thunks call) ───────────────────────────

pub fn open_key(hkey: u64, sub_key: &str) -> Result<u64, u32> {
    HIVE.lock().as_mut().ok_or(6u32)?.open_key(hkey, sub_key)
}

pub fn create_key(hkey: u64, sub_key: &str) -> Result<(u64, bool), u32> {
    HIVE.lock().as_mut().ok_or(6u32)?.create_key(hkey, sub_key)
}

pub fn close_key(handle: u64) -> bool {
    HIVE.lock()
        .as_mut()
        .map(|h| h.close_handle(handle))
        .unwrap_or(false)
}

pub fn query_value(handle: u64, value_name: &str) -> Result<(u32, Vec<u8>), u32> {
    let guard = HIVE.lock();
    let hive = guard.as_ref().ok_or(6u32)?;
    let v = hive.query_value(handle, value_name)?;
    Ok((v.reg_type(), v.to_bytes()))
}

/// Set a value AND mirror it into the versioned config (`/bridge/registry`)
/// — the persistence + rollback story of the whole shim.
pub fn set_value(handle: u64, value_name: &str, value: RegistryValue) -> Result<(), u32> {
    let path = {
        let mut guard = HIVE.lock();
        let hive = guard.as_mut().ok_or(6u32)?;
        let path = hive.handle_path(handle).cloned().ok_or(6u32)?;
        hive.set_value(handle, value_name, value.clone())?;
        path
    }; // HIVE released before config_registry locks its own state.

    if let Some(serialized) = serialize_value(&value) {
        crate::config_registry::set_text(&cfg_key(&path, value_name), &serialized);
        SETS_PERSISTED.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

/// Replay persisted `/bridge/registry/...` values into the (fresh) hive.
fn restore_from_config(hive: &mut RegistryHive) -> u64 {
    let mut restored = 0u64;
    for (cfg_path, text) in crate::config_registry::text_entries_with_prefix(CFG_PREFIX) {
        let rel = &cfg_path[CFG_PREFIX.len()..];
        // Last segment is the value name; the rest is the key path.
        let Some(split) = rel.rfind('/') else {
            continue;
        };
        let (key_path, value_name) = (&rel[..split], &rel[split + 1..]);
        let reg_path = key_path.replace('/', "\\");
        // Root prefix (HKEY_...) is part of the stored path; resolve it by
        // creating relative to the matching predefined root.
        let (root, rest) = match reg_path.split_once('\\') {
            Some((r, rest)) => (r, String::from(rest)),
            None => (reg_path.as_str(), String::new()),
        };
        // Stored paths use the hive's native short prefixes (handle paths
        // resolve to "HKCU\..."); long forms accepted for forward-compat.
        let root_hkey = match root {
            "HKCU" | "HKEY_CURRENT_USER" => raebridge::registry::HKEY_CURRENT_USER,
            "HKLM" | "HKEY_LOCAL_MACHINE" => raebridge::registry::HKEY_LOCAL_MACHINE,
            "HKCR" | "HKEY_CLASSES_ROOT" => raebridge::registry::HKEY_CLASSES_ROOT,
            "HKU" | "HKEY_USERS" => raebridge::registry::HKEY_USERS,
            "HKCC" | "HKEY_CURRENT_CONFIG" => raebridge::registry::HKEY_CURRENT_CONFIG,
            _ => continue,
        };
        let Some(value) = parse_value(&text) else {
            continue;
        };
        if let Ok((h, _)) = hive.create_key(root_hkey, &rest) {
            let _ = hive.set_value(h, value_name, value);
            let _ = hive.close_handle(h);
            restored += 1;
        }
    }
    restored
}

pub fn init() {
    let mut hive = RegistryHive::new();
    let restored = restore_from_config(&mut hive);
    RESTORED_AT_BOOT.store(restored, Ordering::Relaxed);
    *HIVE.lock() = Some(hive);
    crate::serial_println!(
        "[winreg] registry shim up (Windows-10 defaults seeded, {} persisted value(s) replayed from versioned config)",
        restored,
    );
}

/// Deterministic proof of the persistence cycle: set through the shim →
/// mirrored versioned-config key exists → COLD hive (drop + re-init)
/// resurrects the value from config alone → defaults still present →
/// missing keys fail with the right Win32 error.
pub fn run_boot_smoketest() {
    // 1. Write through the shim.
    let set_ok = match create_key(HKEY_CURRENT_USER, "Software\\RaeenSelftest") {
        Ok((h, _)) => {
            let r = set_value(
                h,
                "InstallDir",
                RegistryValue::Sz(String::from("C:\\Games\\Rae")),
            )
            .is_ok()
                && set_value(h, "Launches", RegistryValue::DWord(42)).is_ok();
            close_key(h);
            r
        }
        Err(_) => false,
    };

    // 2. The mirror landed in versioned config (handle paths resolve to the
    // hive's short root prefixes, so the key starts with HKCU).
    let mirrored =
        crate::config_registry::get_text("/bridge/registry/HKCU/Software/RaeenSelftest/InstallDir")
            .as_deref()
            == Some("1:C:\\Games\\Rae");

    // 3. Cold restart: a brand-new hive replays the value from config.
    *HIVE.lock() = None;
    init();
    let resurrected = match open_key(HKEY_CURRENT_USER, "Software\\RaeenSelftest") {
        Ok(h) => {
            let s = query_value(h, "InstallDir")
                .map(|(t, b)| t == raebridge::registry::REG_SZ && !b.is_empty())
                .unwrap_or(false);
            let d = query_value(h, "Launches")
                .map(|(t, b)| t == raebridge::registry::REG_DWORD && b == 42u32.to_le_bytes())
                .unwrap_or(false);
            close_key(h);
            s && d
        }
        Err(_) => false,
    };

    // 4. The Windows-10 default keys apps probe are present.
    let defaults_ok = open_key(
        HKEY_LOCAL_MACHINE,
        "Software\\Microsoft\\Windows NT\\CurrentVersion",
    )
    .map(|h| {
        let ok = query_value(h, "ProductName").is_ok();
        close_key(h);
        ok
    })
    .unwrap_or(false);

    // 5. Win32 error semantics: missing key = ERROR_FILE_NOT_FOUND (2).
    let missing_ok = open_key(HKEY_CURRENT_USER, "Software\\DoesNotExist") == Err(2);

    let pass = set_ok && mirrored && resurrected && defaults_ok && missing_ok;
    crate::serial_println!(
        "[winreg] smoketest: set={} versioned_mirror={} cold_restore={} win10_defaults={} err_semantics={} -> {}",
        set_ok,
        mirrored,
        resurrected,
        defaults_ok,
        missing_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/winreg` — registry shim state.
pub fn dump_text() -> String {
    alloc::format!(
        "# Win32 registry shim (backed by versioned config under /bridge/registry)\nsets_persisted: {}\nrestored_at_boot: {}\nhive_loaded: {}\n",
        SETS_PERSISTED.load(Ordering::Relaxed),
        RESTORED_AT_BOOT.load(Ordering::Relaxed),
        HIVE.lock().is_some(),
    )
}
