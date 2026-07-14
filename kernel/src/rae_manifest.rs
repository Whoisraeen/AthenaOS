//! `RaeManifest.toml` — per-app permission manifests (RaeShield, Phase 9).
//!
//! Concept §Security:
//! > "Capability-based permissions — apps request capabilities (file access,
//! >  camera, mic, network), user grants, OS enforces at the syscall layer."
//! > "Mandatory app sandboxing — every app runs in its own sandbox by default."
//!
//! This module is the *declaration* side of that model. Every app bundle may
//! carry an `apps/<name>/RaeManifest.toml` in the boot media (initramfs today,
//! RaeFS app bundles later) stating which sandbox level it runs at and which
//! gated syscall classes it requests:
//!
//! ```toml
//! name = "calculator"
//! version = "1.0.0"
//! sandbox = "app"          # trusted | app | strict
//!
//! [permissions]
//! network = true           # socket syscalls (121/122)
//! devices = false          # raw device claim / DMA / driver registration
//! install = false          # disk install (SYS_INSTALL_*)
//! ```
//!
//! At launch the spawn path calls [`assign_for_spawn`], which parses the
//! manifest and applies it via `sandbox::set_task_level` +
//! `sandbox::set_task_grants`. Apps without a manifest fall back to the
//! first-party allowlist (`sandbox::level_for_app`): known system apps run
//! Trusted, everything else gets AppSandbox.
//!
//! ## Code signing (Phase 9.2)
//!
//! xtask signs each staged manifest at build time: it injects the built
//! ELF's `elf_sha256`, Ed25519-signs the staged bytes with the dev key in
//! `keys/dev-signing.key`, and bundles the detached `RaeManifest.sig`.
//! [`lookup`] verifies the signature against the embedded
//! [`DEV_SIGNING_PUBKEY`] and the ELF hash against the binary in the same
//! bundle. Outcomes:
//!   * valid signature + matching ELF hash → `verified = true` (store-app
//!     posture; the signature is a sufficient trust root for `trusted`),
//!   * no signature → `verified = false` (the "unverified developer"
//!     sideload posture; trust rules below apply),
//!   * BAD signature / hash mismatch / signed-without-hash → the whole
//!     bundle is rejected (fail-close: tampered ≠ unsigned).
//!
//! ## Trust rules
//!
//! An *unsigned* manifest must not be able to escalate the app:
//!   * `sandbox = "trusted"` is honored only with a trust root: a verified
//!     signature, or membership on the kernel's first-party allowlist;
//!     anyone else is capped to AppSandbox and the cap is logged.
//!   * `[permissions]` grants apply only at AppSandbox. Strict apps (the
//!     unverified-sideload posture) never receive grants — fail-close.
//!   * The manifest's `name` must match its bundle directory; a mismatch is
//!     treated as no-manifest (spoof guard).
//!
//! The parser is a deliberate TOML *subset* (comments, `[section]`,
//! `key = "string" | true | false`) — enough for the schema above, no general
//! TOML machinery in the kernel. Unknown keys and sections are ignored for
//! forward compatibility; malformed lines fail the whole parse (fail-close to
//! the allowlist fallback).
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/manifests` + this
//! docstring.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::sandbox::{Grants, SandboxLevel};

/// The DEV bundle-signing public key (Ed25519), generated and kept in
/// lockstep by xtask (`keys/dev-signing.pub`). xtask signs each staged
/// manifest with the matching seed at build time. This is a development
/// trust root — "free signing" for every dev build (Concept §Developer
/// onramp); the production chain (HSM keys, per-developer certs) replaces it
/// at store onboarding / Phase 3.7.
static DEV_SIGNING_PUBKEY: [u8; 32] = *include_bytes!("../../keys/dev-signing.pub");

/// A parsed `RaeManifest.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaeManifest {
    pub name: String,
    pub version: String,
    pub sandbox: SandboxLevel,
    pub grants: Grants,
    /// sha256 of the app's ELF, injected + signed by xtask at build time.
    /// Required for signed bundles (binds the signature to the binary).
    pub elf_sha256: Option<[u8; 32]>,
    /// True when the bundle carried a RaeManifest.sig that verified against
    /// [`DEV_SIGNING_PUBKEY`] AND the ELF hash matched. Set by [`lookup`],
    /// never by [`parse`].
    pub verified: bool,
}

impl Default for RaeManifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            version: String::new(),
            // Declaring a manifest without a level means "sandbox me":
            // AppSandbox is the safe default, NOT Trusted.
            sandbox: SandboxLevel::AppSandbox,
            grants: Grants::default(),
            elf_sha256: None,
            verified: false,
        }
    }
}

/// sha256 of `data` via the kernel's own primitive (KAT-proven, crypto.rs).
fn sha256(data: &[u8]) -> [u8; 32] {
    use crate::crypto::HashAlgorithm;
    let mut ctx = crate::crypto::Sha256Context::new();
    ctx.update(data);
    let mut out = [0u8; 32];
    ctx.finalize(&mut out);
    out
}

/// Manifests discovered in the boot media at init (count only; lookups are
/// always live against the tar so procfs reflects reality, not a cache).
static DISCOVERED: AtomicU64 = AtomicU64::new(0);
/// Spawns that took the manifest path vs the allowlist fallback.
static SPAWNS_FROM_MANIFEST: AtomicU64 = AtomicU64::new(0);
static SPAWNS_FROM_FALLBACK: AtomicU64 = AtomicU64::new(0);
/// Manifests that declared `trusted` without first-party standing (capped).
static TRUST_CAPS: AtomicU64 = AtomicU64::new(0);

// ── TOML-subset parser ──────────────────────────────────────────────────

/// Strip a `#` comment, respecting `"`-quoted strings.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Parse a quoted TOML string value: `"text"`.
fn parse_str(val: &str) -> Result<String, &'static str> {
    let v = val.trim();
    let inner = v
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or("expected \"quoted string\"")?;
    if inner.contains('"') {
        return Err("embedded quote in string");
    }
    Ok(String::from(inner))
}

/// Parse a TOML boolean: `true` | `false`.
fn parse_bool(val: &str) -> Result<bool, &'static str> {
    match val.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("expected true|false"),
    }
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a 64-hex-char quoted string into 32 bytes (the elf_sha256 value).
fn parse_hex32(val: &str) -> Result<[u8; 32], &'static str> {
    let s = parse_str(val)?;
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return Err("elf_sha256 must be 64 hex chars");
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_nibble(bytes[2 * i]).ok_or("invalid hex in elf_sha256")?;
        let lo = hex_nibble(bytes[2 * i + 1]).ok_or("invalid hex in elf_sha256")?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

/// Parse `RaeManifest.toml` bytes. Fail-close: any malformed line rejects the
/// whole manifest (the caller falls back to the allowlist).
pub fn parse(bytes: &[u8]) -> Result<RaeManifest, &'static str> {
    let text = core::str::from_utf8(bytes).map_err(|_| "not utf-8")?;
    let mut m = RaeManifest::default();
    // "" = top level, "permissions" = [permissions], "*" = unknown (ignored).
    let mut section = "";
    let mut saw_key = false;

    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').ok_or("malformed [section]")?;
            section = match name.trim() {
                "permissions" => "permissions",
                _ => "*", // forward-compat: unknown section, keys ignored
            };
            continue;
        }
        let eq = line.find('=').ok_or("expected key = value")?;
        let key = line[..eq].trim();
        let val = &line[eq + 1..];
        saw_key = true;
        match (section, key) {
            ("", "name") => m.name = parse_str(val)?,
            ("", "version") => m.version = parse_str(val)?,
            ("", "sandbox") => {
                m.sandbox = match parse_str(val)?.as_str() {
                    "trusted" => SandboxLevel::Trusted,
                    "app" => SandboxLevel::AppSandbox,
                    "strict" => SandboxLevel::Strict,
                    _ => return Err("sandbox must be trusted|app|strict"),
                }
            }
            ("", "elf_sha256") => m.elf_sha256 = Some(parse_hex32(val)?),
            ("permissions", "network") => m.grants.network = parse_bool(val)?,
            ("permissions", "devices") => m.grants.devices = parse_bool(val)?,
            ("permissions", "install") => m.grants.install = parse_bool(val)?,
            // Unknown key in a known/unknown section: forward-compat ignore.
            _ => {}
        }
    }

    if !saw_key {
        return Err("empty manifest");
    }
    if m.name.is_empty() {
        return Err("missing name");
    }
    Ok(m)
}

// ── Bundle lookup ───────────────────────────────────────────────────────

fn manifest_path(app: &str) -> String {
    alloc::format!("apps/{}/RaeManifest.toml", app)
}

fn signature_path(app: &str) -> String {
    alloc::format!("apps/{}/RaeManifest.sig", app)
}

/// Look up, parse, and authenticate the manifest for `app` from the boot
/// initramfs. Returns `None` (→ allowlist fallback) when absent, malformed,
/// or when the declared `name` doesn't match the bundle directory (spoof
/// guard).
///
/// Signing (Phase 9.2 code signing): when `RaeManifest.sig` is present it
/// MUST verify against [`DEV_SIGNING_PUBKEY`] over the exact manifest bytes,
/// the manifest MUST carry `elf_sha256`, and that hash MUST match the app's
/// ELF in the same bundle — any failure rejects the whole bundle lookup
/// (fail-close: a tampered store bundle must not fall back to looking
/// "unsigned but fine"). A bundle with NO .sig parses as unsigned
/// (`verified=false`) — the "unverified developer" sideload posture.
pub fn lookup(app: &str) -> Option<RaeManifest> {
    let archive = crate::tar::TarArchive::new(crate::INITRAMFS);
    let file = archive.get_file(&manifest_path(app))?;
    let mut m = match parse(file.data) {
        Ok(m) => m,
        Err(e) => {
            crate::serial_println!("[manifest] '{}': parse error ({}) — ignored", app, e);
            return None;
        }
    };
    if m.name != app {
        crate::serial_println!(
            "[manifest] '{}': name '{}' doesn't match bundle dir — ignored",
            app,
            m.name,
        );
        return None;
    }
    if let Some(sig_file) = archive.get_file(&signature_path(app)) {
        if sig_file.data.len() != 64 {
            crate::serial_println!(
                "[manifest] '{}': malformed signature ({} bytes) — bundle rejected",
                app,
                sig_file.data.len(),
            );
            return None;
        }
        let mut sig = [0u8; 64];
        sig.copy_from_slice(sig_file.data);
        let ctx = crate::crypto::Ed25519Context::with_public_key(DEV_SIGNING_PUBKEY);
        if !matches!(ctx.verify(file.data, &sig), Ok(true)) {
            crate::serial_println!("[manifest] '{}': BAD SIGNATURE — bundle rejected", app);
            return None;
        }
        // Signature is over the manifest; elf_sha256 binds it to the binary.
        let Some(expected) = m.elf_sha256 else {
            crate::serial_println!(
                "[manifest] '{}': signed manifest lacks elf_sha256 — bundle rejected",
                app
            );
            return None;
        };
        let Some(elf) = archive.get_file(app) else {
            crate::serial_println!(
                "[manifest] '{}': signed bundle has no ELF in boot media — rejected",
                app
            );
            return None;
        };
        if sha256(elf.data) != expected {
            crate::serial_println!(
                "[manifest] '{}': ELF hash mismatch — tampered bundle rejected",
                app
            );
            return None;
        }
        m.verified = true;
    }
    Some(m)
}

/// Apply the trust rules: a manifest may self-declare `app`/`strict` freely,
/// but `trusted` requires a trust root — either a verified bundle signature
/// (code signing) or first-party standing (the kernel allowlist).
fn effective_level(app: &str, declared: SandboxLevel, verified: bool) -> SandboxLevel {
    if declared == SandboxLevel::Trusted
        && !verified
        && crate::sandbox::level_for_app(app) != SandboxLevel::Trusted
    {
        TRUST_CAPS.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!(
            "[manifest] '{}' declares sandbox=\"trusted\" unsigned and without first-party standing — capped to AppSandbox",
            app,
        );
        return SandboxLevel::AppSandbox;
    }
    declared
}

/// The spawn-path entry point: assign `pid` its sandbox level (and grants)
/// from the app's manifest, falling back to the first-party allowlist when no
/// valid manifest exists. Returns `(level, from_manifest)` for the launch log.
pub fn assign_for_spawn(app: &str, pid: u64) -> (SandboxLevel, bool) {
    match lookup(app) {
        Some(m) => {
            let level = effective_level(app, m.sandbox, m.verified);
            crate::sandbox::set_task_level(pid, level);
            // Grants only mean something at AppSandbox (Trusted needs none,
            // Strict must not receive any — fail-close).
            let grants = if level == SandboxLevel::AppSandbox {
                m.grants
            } else {
                Grants::default()
            };
            crate::sandbox::set_task_grants(pid, grants);
            if grants != Grants::default() {
                crate::serial_println!(
                    "[manifest] '{}' pid {} grants: net={} dev={} install={}",
                    app,
                    pid,
                    grants.network,
                    grants.devices,
                    grants.install,
                );
            }
            SPAWNS_FROM_MANIFEST.fetch_add(1, Ordering::Relaxed);
            (level, true)
        }
        None => {
            let level = crate::sandbox::level_for_app(app);
            crate::sandbox::set_task_level(pid, level);
            SPAWNS_FROM_FALLBACK.fetch_add(1, Ordering::Relaxed);
            (level, false)
        }
    }
}

/// Names of all app bundles in the initramfs that carry a manifest.
fn discovered_apps() -> Vec<String> {
    let archive = crate::tar::TarArchive::new(crate::INITRAMFS);
    let mut out = Vec::new();
    for f in archive.iter() {
        if let Some(rest) = f.name.strip_prefix("apps/") {
            if let Some(app) = rest.strip_suffix("/RaeManifest.toml") {
                if !app.is_empty() && !app.contains('/') {
                    out.push(String::from(app));
                }
            }
        }
    }
    out
}

pub fn init() {
    let apps = discovered_apps();
    DISCOVERED.store(apps.len() as u64, Ordering::Relaxed);
    crate::serial_println!(
        "[ OK ] RaeManifest registry: {} app manifest(s) in boot media",
        apps.len(),
    );
}

// ── Boot smoketest ──────────────────────────────────────────────────────

/// Prove the whole chain at boot with the real initramfs manifests:
/// parse → signature + ELF-hash verification → trust rules → tamper
/// rejection → spawn assignment → live syscall-gate effect.
pub fn run_boot_smoketest() {
    // 1. The shipped calculator manifest parses, declares AppSandbox, and its
    //    build-time signature + ELF-hash binding verify (code signing).
    let cal = lookup("calculator");
    let found = cal.is_some();
    let parsed = cal
        .as_ref()
        .map(|m| m.sandbox == SandboxLevel::AppSandbox && m.grants.network)
        .unwrap_or(false);
    let signed = cal.as_ref().map(|m| m.verified).unwrap_or(false);

    // 2. Trust rules: unsigned non-first-party "trusted" is capped, while a
    //    verified signature is a sufficient trust root on its own.
    let capped = effective_level("totally_unknown_app", SandboxLevel::Trusted, false)
        == SandboxLevel::AppSandbox;
    let signed_trust = effective_level("totally_unknown_app", SandboxLevel::Trusted, true)
        == SandboxLevel::Trusted;

    // 3. Garbage input is rejected (fail-close to allowlist fallback).
    let reject_garbage = parse(b"this is not a manifest").is_err() && parse(b"\xff\xfe").is_err();

    // 4. Tamper rejection: flip one byte of the signed manifest and the
    //    signature must no longer verify.
    let reject_tamper = {
        let archive = crate::tar::TarArchive::new(crate::INITRAMFS);
        match (
            archive.get_file(&manifest_path("calculator")),
            archive.get_file(&signature_path("calculator")),
        ) {
            (Some(mf), Some(sf)) if sf.data.len() == 64 => {
                let mut tampered = alloc::vec::Vec::from(mf.data);
                tampered[0] ^= 0x01;
                let mut sig = [0u8; 64];
                sig.copy_from_slice(sf.data);
                let ctx = crate::crypto::Ed25519Context::with_public_key(DEV_SIGNING_PUBKEY);
                matches!(ctx.verify(&tampered, &sig), Ok(false))
            }
            _ => false,
        }
    };

    // 5. Spawn assignment on a synthetic pid: calculator's manifest grants
    //    network (sockets allowed) but NOT devices (claim denied) — the
    //    permission manifest visibly drives the live syscall gate.
    let probe_pid: u64 = 0x5A_FE_00_03;
    let (level, from_manifest) = assign_for_spawn("calculator", probe_pid);
    let net_allowed = crate::sandbox::check_syscall(probe_pid, 121);
    let dev_denied =
        !crate::sandbox::check_syscall(probe_pid, rae_abi::syscall::SYS_DRIVER_CLAIM_DEVICE);
    let assign = from_manifest && level == SandboxLevel::AppSandbox && net_allowed && dev_denied;
    crate::sandbox::forget_task(probe_pid);

    let pass = found
        && parsed
        && signed
        && capped
        && signed_trust
        && reject_garbage
        && reject_tamper
        && assign;
    crate::serial_println!(
        "[manifest] run_boot_smoketest: found={} parsed={} signed={} trust_cap={} signed_trust={} reject_garbage={} reject_tamper={} spawn_gate={} -> {}",
        found,
        parsed,
        signed,
        capped,
        signed_trust,
        reject_garbage,
        reject_tamper,
        assign,
        if pass { "PASS" } else { "FAIL" },
    );
}

// ── procfs ──────────────────────────────────────────────────────────────

pub fn dump_text() -> String {
    let mut out = String::from("# RaeenOS app permission manifests (RaeManifest.toml)\n");
    out.push_str(&alloc::format!(
        "discovered: {}\nspawns_from_manifest: {}\nspawns_from_fallback: {}\ntrust_caps: {}\n",
        DISCOVERED.load(Ordering::Relaxed),
        SPAWNS_FROM_MANIFEST.load(Ordering::Relaxed),
        SPAWNS_FROM_FALLBACK.load(Ordering::Relaxed),
        TRUST_CAPS.load(Ordering::Relaxed),
    ));
    for app in discovered_apps() {
        match lookup(&app) {
            Some(m) => out.push_str(&alloc::format!(
                "  {} v{} sandbox={:?} signed={} net={} dev={} install={}\n",
                m.name,
                m.version,
                m.sandbox,
                m.verified,
                m.grants.network,
                m.grants.devices,
                m.grants.install,
            )),
            None => out.push_str(&alloc::format!("  {} (invalid/tampered — rejected)\n", app)),
        }
    }
    out
}
