use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct OsProfile {
    pub packages: BTreeMap<String, toml::Value>,
}

pub fn parse_profile(path: &Path) -> Vec<String> {
    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!(
            "[xtask] Failed to read OS profile at {}: {}",
            path.display(),
            e
        );
        std::process::exit(1);
    });

    let profile: OsProfile = toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("[xtask] Failed to parse OS profile TOML: {}", e);
        std::process::exit(1);
    });

    profile.packages.keys().cloned().collect()
}

/// Packages declaring `abi = "linux"` in the profile. xtask stamps these
/// ELFOSABI_LINUX (0x03) instead of ELFOSABI_ATHENAOS, so the kernel's
/// SYS_SPAWN routes them through linux_exec (Linux auxv stack + Linux
/// syscall table). Everything else in the profile is native by construction
/// — including relibc-linked apps, whose relibc port speaks NATIVE AthenaOS
/// syscall numbers (components/athbridge/relibc/src/athenaOS_syscall.rs).
pub fn parse_profile_linux_abi(path: &Path) -> Vec<String> {
    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!(
            "[xtask] Failed to read OS profile at {}: {}",
            path.display(),
            e
        );
        std::process::exit(1);
    });
    let profile: OsProfile = toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("[xtask] Failed to parse OS profile TOML: {}", e);
        std::process::exit(1);
    });
    profile
        .packages
        .iter()
        .filter(|(_, v)| v.get("abi").and_then(|a| a.as_str()) == Some("linux"))
        .map(|(k, _)| k.clone())
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct Recipe {
    pub source: SourceConfig,
    pub build: Option<BuildConfig>,
}

#[derive(Debug, Deserialize)]
pub struct SourceConfig {
    pub git: Option<String>,
    pub branch: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BuildConfig {
    pub template: Option<String>,
    pub script: Option<String>,
}

pub fn parse_port_recipe(path: &Path) -> Recipe {
    let content = fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!(
            "[xtask] Failed to read port recipe at {}: {}",
            path.display(),
            e
        );
        std::process::exit(1);
    });

    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("[xtask] Failed to parse port recipe TOML: {}", e);
        std::process::exit(1);
    })
}
