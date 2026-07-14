//! App bundle manifest verifier — Concept §Windows pain points:
//!
//! > "DLL hell → App bundles with explicit, hashed dependencies."
//!
//! Every Windows installer answers the question "what does this app need
//! to run?" with some flavor of "I'll drop a few DLLs in System32 and
//! pray no one else changes them." Decades of side-by-side assemblies,
//! manifests, redistributables, and "vc_redist.x64.exe" exist to paper
//! over this and still don't really solve it.
//!
//! RaeenOS makes the dependency graph explicit and signed. Every installed
//! component (a shared library, a framework, a runtime) registers itself
//! with a `(name, version, sha256)` triple. An app's `.raeapp` manifest
//! declares its dependencies the same way. Before the kernel will let an
//! app launch, the loader (or a userspace `raepackage` daemon) calls
//! `SYS_BUNDLE_VERIFY` with the manifest and the kernel either says "all
//! deps are installed at the requested hashes" or "missing: libfoo 1.2.3
//! sha256:abc…".
//!
//! That's it. No DLL hell, no PATH wars, no "this works on my machine."
//!
//! ## On-wire manifest format
//!
//! A flat, self-describing byte layout — no JSON parser in kernel.
//!
//! ```text
//!   off  0   u32   magic = 0x52454250 ('REBP' = "RaeEnv Bundled Package")
//!   off  4   u32   version = 1
//!   off  8   u32   name_len  (UTF-8, no NUL)
//!   off 12   u32   app_version (packed semver: maj<<16 | min<<8 | patch)
//!   off 16   u32   dep_count
//!   off 20   u8[name_len]  app name
//!   off ...  Dep[dep_count]
//!
//!   struct Dep {
//!       u32  name_len;
//!       u32  required_version;  // same packed semver
//!       u8   sha256[32];
//!       u8[name_len] name;
//!   }
//! ```
//!
//! Returns a packed result word from `SYS_BUNDLE_VERIFY`:
//! ```text
//!   bits  0..16  = number of dependencies verified ok
//!   bits 16..32  = number of dependencies missing or mismatched
//!   bits 32..40  = top-level error code (0 = parse ok)
//!   bits 40..64  = reserved
//! ```
//!
//! ## Syscalls (66-67)
//!
//! | nr | name             | rdi/rsi/rdx/r10                                   | rax |
//! |----|------------------|---------------------------------------------------|----|
//! | 66 | BUNDLE_VERIFY    | rdi=manifest_ptr, rsi=manifest_len                | packed result |
//! | 67 | BUNDLE_REGISTER  | rdi=name_ptr, rsi=name_len, rdx=ver, r10=sha_ptr  | 0/err |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

// ── Component registry ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Component {
    name: String,
    version: u32,
    sha256: [u8; 32],
}

struct Registry {
    /// Indexed by name, then version → component.
    by_name: BTreeMap<String, BTreeMap<u32, Component>>,
    /// Aggregate: number of verifications attempted since boot.
    verifications: u64,
    /// Verifications that returned "all deps OK".
    verifications_passed: u64,
}

impl Registry {
    fn new() -> Self {
        let mut r = Self {
            by_name: BTreeMap::new(),
            verifications: 0,
            verifications_passed: 0,
        };
        r.seed();
        r
    }

    fn seed(&mut self) {
        // Pre-register the core RaeenOS frameworks so that any app whose
        // manifest declares them as dependencies verifies cleanly out of
        // the box. SHA-256 values are deterministic deriv-from-name
        // placeholders — the real package server will replace these with
        // actual build artifacts.
        let core: &[(&str, u32)] = &[
            ("raegfx", pack_semver(0, 1, 0)),
            ("raeui", pack_semver(0, 1, 0)),
            ("raekit", pack_semver(0, 1, 0)),
            ("raeaudio", pack_semver(0, 1, 0)),
            ("raenet", pack_semver(0, 1, 0)),
            ("raeshield", pack_semver(0, 1, 0)),
            ("raefont", pack_semver(0, 1, 0)),
            ("raestore", pack_semver(0, 1, 0)),
            ("raeplay", pack_semver(0, 1, 0)),
            ("raebridge", pack_semver(0, 1, 0)),
            ("rust-std", pack_semver(1, 80, 0)),
            ("libc-rae", pack_semver(1, 0, 0)),
        ];
        for (name, ver) in core {
            let mut hash = [0u8; 32];
            // Cheap deterministic stand-in: byte 0 is "core flag", rest
            // is a rolling pattern of name bytes XOR'd with version.
            hash[0] = 0xCC;
            for (i, b) in name.as_bytes().iter().enumerate() {
                hash[1 + (i % 31)] ^= b ^ ((*ver as u8).wrapping_add(i as u8));
            }
            self.by_name
                .entry(String::from(*name))
                .or_insert_with(BTreeMap::new)
                .insert(
                    *ver,
                    Component {
                        name: String::from(*name),
                        version: *ver,
                        sha256: hash,
                    },
                );
        }
    }

    fn register(&mut self, comp: Component) {
        self.by_name
            .entry(comp.name.clone())
            .or_insert_with(BTreeMap::new)
            .insert(comp.version, comp);
    }

    /// Looks up a component by (name, version). Returns the stored hash
    /// if installed, else None.
    fn lookup_hash(&self, name: &str, version: u32) -> Option<&[u8; 32]> {
        self.by_name
            .get(name)
            .and_then(|by_ver| by_ver.get(&version))
            .map(|c| &c.sha256)
    }
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

// ── Parse + verify ─────────────────────────────────────────────────────

const MAGIC: u32 = 0x52454250; // "REBP"

#[derive(Debug)]
pub struct ManifestDep {
    pub name: String,
    pub version: u32,
    pub sha256: [u8; 32],
}

#[derive(Debug)]
pub struct Manifest {
    pub name: String,
    pub app_version: u32,
    pub deps: Vec<ManifestDep>,
}

/// Manifest-level parse errors (encoded in bits 32..40 of the verify rax).
pub const PARSE_OK: u8 = 0;
pub const PARSE_TRUNCATED: u8 = 1;
pub const PARSE_BAD_MAGIC: u8 = 2;
pub const PARSE_BAD_VERSION: u8 = 3;
pub const PARSE_TOO_LARGE: u8 = 4;
pub const PARSE_BAD_UTF8: u8 = 5;

pub fn parse(bytes: &[u8]) -> Result<Manifest, u8> {
    if bytes.len() < 20 {
        return Err(PARSE_TRUNCATED);
    }
    let rd_u32 = |off: usize| {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    };
    let magic = rd_u32(0);
    let version = rd_u32(4);
    let name_len = rd_u32(8) as usize;
    let app_version = rd_u32(12);
    let dep_count = rd_u32(16) as usize;
    if magic != MAGIC {
        return Err(PARSE_BAD_MAGIC);
    }
    if version != 1 {
        return Err(PARSE_BAD_VERSION);
    }
    if name_len > 256 {
        return Err(PARSE_TOO_LARGE);
    }
    if dep_count > 512 {
        return Err(PARSE_TOO_LARGE);
    }
    if bytes.len() < 20 + name_len {
        return Err(PARSE_TRUNCATED);
    }

    let name_bytes = &bytes[20..20 + name_len];
    let name = core::str::from_utf8(name_bytes)
        .map_err(|_| PARSE_BAD_UTF8)?
        .to_string();

    let mut cursor = 20 + name_len;
    let mut deps = Vec::with_capacity(dep_count);
    for _ in 0..dep_count {
        if cursor + 40 > bytes.len() {
            return Err(PARSE_TRUNCATED);
        }
        let dn_len = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as usize;
        let req_ver = u32::from_le_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]);
        if dn_len > 128 {
            return Err(PARSE_TOO_LARGE);
        }
        let mut sha = [0u8; 32];
        sha.copy_from_slice(&bytes[cursor + 8..cursor + 40]);
        let nstart = cursor + 40;
        if nstart + dn_len > bytes.len() {
            return Err(PARSE_TRUNCATED);
        }
        let dn = core::str::from_utf8(&bytes[nstart..nstart + dn_len])
            .map_err(|_| PARSE_BAD_UTF8)?
            .to_string();
        deps.push(ManifestDep {
            name: dn,
            version: req_ver,
            sha256: sha,
        });
        cursor = nstart + dn_len;
    }

    Ok(Manifest {
        name,
        app_version,
        deps,
    })
}

/// Verify a manifest against the installed registry. Returns
/// `(ok_count, missing_count, mismatches_list_of_names)`.
pub fn verify(manifest: &Manifest) -> (u32, u32, Vec<String>) {
    let mut g = REGISTRY.lock();
    let reg = match g.as_mut() {
        Some(r) => r,
        None => return (0, manifest.deps.len() as u32, Vec::new()),
    };
    reg.verifications += 1;
    let mut ok = 0u32;
    let mut bad = 0u32;
    let mut bad_names = Vec::new();
    for dep in &manifest.deps {
        let installed = reg.lookup_hash(&dep.name, dep.version);
        match installed {
            Some(h) if *h == dep.sha256 => ok += 1,
            _ => {
                bad += 1;
                bad_names.push(dep.name.clone());
            }
        }
    }
    if bad == 0 {
        reg.verifications_passed += 1;
    }
    (ok, bad, bad_names)
}

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    let reg = Registry::new();
    let n = reg.by_name.len();
    let total_versions: usize = reg.by_name.values().map(|m| m.len()).sum();
    *REGISTRY.lock() = Some(reg);
    crate::serial_println!(
        "[ OK ] App bundle verifier: {} component(s), {} version(s) registered",
        n,
        total_versions,
    );
}

// ── Syscall handlers ───────────────────────────────────────────────────

pub const SYS_BUNDLE_VERIFY: u64 = 66;
pub const SYS_BUNDLE_REGISTER: u64 = 67;

const MAX_MANIFEST_BYTES: u64 = 65_536;

pub fn sys_verify(
    manifest_ptr: u64,
    manifest_len: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if manifest_len == 0 || manifest_len > MAX_MANIFEST_BYTES {
        return pack(0, 0, PARSE_TOO_LARGE);
    }
    if !validate_r(manifest_ptr, manifest_len, false) {
        return pack(0, 0, PARSE_TRUNCATED);
    }
    let bytes = read_user_bytes(manifest_ptr, manifest_len);
    let manifest = match parse(&bytes) {
        Ok(m) => m,
        Err(e) => return pack(0, 0, e),
    };
    let (ok, bad, bad_names) = verify(&manifest);
    if bad > 0 {
        crate::serial_println!(
            "[bundle] verify '{}' v{:x}: {} ok, {} bad ({})",
            manifest.name,
            manifest.app_version,
            ok,
            bad,
            bad_names
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    pack(ok, bad, PARSE_OK)
}

pub fn sys_register(
    name_ptr: u64,
    name_len: u64,
    version: u64,
    sha_ptr: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(name_ptr, name_len, false) {
        return u64::MAX;
    }
    if !validate_r(sha_ptr, 32, false) {
        return u64::MAX;
    }
    let name = read_user_string(name_ptr, name_len);
    let mut sha = [0u8; 32];
    // Validated + fault-fixup copy of the 32-byte digest.
    match crate::uaccess::copy_from_user(sha_ptr, 32) {
        Ok(v) if v.len() == 32 => sha.copy_from_slice(&v),
        _ => return u64::MAX,
    }
    let mut g = REGISTRY.lock();
    if let Some(reg) = g.as_mut() {
        reg.register(Component {
            name,
            version: version as u32,
            sha256: sha,
        });
        0
    } else {
        u64::MAX
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

const fn pack_semver(maj: u8, min: u8, patch: u8) -> u32 {
    ((maj as u32) << 16) | ((min as u32) << 8) | (patch as u32)
}

fn pack(ok: u32, bad: u32, err: u8) -> u64 {
    (ok as u64 & 0xFFFF) | ((bad as u64 & 0xFFFF) << 16) | ((err as u64 & 0xFF) << 32)
}

fn read_user_string(ptr: u64, len: u64) -> String {
    // Validated + fault-fixup (was a raw arbitrary-kernel-read).
    crate::uaccess::read_user_string(ptr, len)
}

fn read_user_bytes(ptr: u64, len: u64) -> Vec<u8> {
    crate::uaccess::read_user_bytes(ptr, len)
}

// ── /proc/raeen/bundles ────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return String::from("# app bundle registry not initialized\n"),
    };
    let mut out = String::new();
    let total: usize = reg.by_name.values().map(|m| m.len()).sum();
    out.push_str(&alloc::format!(
        "# RaeenOS installed components ({} unique, {} total versions, {} verifications, {} passed)\n",
        reg.by_name.len(), total,
        reg.verifications, reg.verifications_passed,
    ));
    for (name, by_ver) in &reg.by_name {
        for (v, comp) in by_ver {
            out.push_str(&alloc::format!(
                "{:20} v{}.{}.{}  sha256:{}\n",
                name,
                (v >> 16) & 0xFF,
                (v >> 8) & 0xFF,
                v & 0xFF,
                hex32(&comp.sha256),
            ));
        }
    }
    out
}

fn hex32(b: &[u8; 32]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(8);
    // Show first 4 bytes as 8 hex chars — full hashes are pointless in
    // a `cat` display.
    for byte in &b[..4] {
        s.push(H[((*byte >> 4) & 0xF) as usize] as char);
        s.push(H[(*byte & 0xF) as usize] as char);
    }
    s.push('…');
    s
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Build a tiny in-memory manifest and run it through parse+verify so
    // the boot log shows the round trip works end-to-end.
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    buf.extend_from_slice(&MAGIC.to_le_bytes());
    buf.extend_from_slice(&1u32.to_le_bytes()); // version
    let app_name = b"smoketest_app";
    buf.extend_from_slice(&(app_name.len() as u32).to_le_bytes());
    buf.extend_from_slice(&pack_semver(0, 1, 0).to_le_bytes()); // app version
    buf.extend_from_slice(&2u32.to_le_bytes()); // dep_count
    buf.extend_from_slice(app_name);

    // Dep 0: raegfx 0.1.0 with the correct hash → should verify OK.
    let raegfx_hash = {
        let g = REGISTRY.lock();
        g.as_ref()
            .and_then(|r| r.lookup_hash("raegfx", pack_semver(0, 1, 0)).copied())
            .unwrap_or([0u8; 32])
    };
    let d0 = b"raegfx";
    buf.extend_from_slice(&(d0.len() as u32).to_le_bytes());
    buf.extend_from_slice(&pack_semver(0, 1, 0).to_le_bytes());
    buf.extend_from_slice(&raegfx_hash);
    buf.extend_from_slice(d0);

    // Dep 1: libfoo 1.0.0 that's NOT installed — should report missing.
    let d1 = b"libfoo";
    buf.extend_from_slice(&(d1.len() as u32).to_le_bytes());
    buf.extend_from_slice(&pack_semver(1, 0, 0).to_le_bytes());
    buf.extend_from_slice(&[0u8; 32]);
    buf.extend_from_slice(d1);

    let m = match parse(&buf) {
        Ok(m) => m,
        Err(e) => {
            crate::serial_println!("[bundle] smoketest parse failed code={}", e);
            return;
        }
    };
    let (ok, bad, _) = verify(&m);
    let happy_ok = m.name == "smoketest_app" && m.deps.len() == 2 && ok == 1 && bad == 1;

    // ── Hostile-input rejection: a manifest parser on the code-signing path
    //    MUST fail-closed on malformed/crafted bytes, never OOB-read or accept
    //    garbage. Each case must return the SPECIFIC error, not just "an" error.
    let rej = |bytes: &[u8], want: u8| matches!(parse(bytes), Err(e) if e == want);

    // Truncated (< 20-byte header).
    let r_trunc = rej(&[0u8; 8], PARSE_TRUNCATED);
    // Bad magic.
    let mut bad_magic = buf.clone();
    bad_magic[0] ^= 0xFF;
    let r_magic = rej(&bad_magic, PARSE_BAD_MAGIC);
    // Unsupported version.
    let mut bad_ver = buf.clone();
    bad_ver[4] = 99;
    let r_ver = rej(&bad_ver, PARSE_BAD_VERSION);
    // name_len beyond the cap (claims 100000-byte name).
    let mut huge_name = buf.clone();
    huge_name[8..12].copy_from_slice(&100_000u32.to_le_bytes());
    let r_toolarge = rej(&huge_name, PARSE_TOO_LARGE);
    // name_len within the cap but past the buffer end (truncated name body).
    let mut short_body = Vec::new();
    short_body.extend_from_slice(&MAGIC.to_le_bytes());
    short_body.extend_from_slice(&1u32.to_le_bytes());
    short_body.extend_from_slice(&200u32.to_le_bytes()); // name_len=200, but...
    short_body.extend_from_slice(&0u32.to_le_bytes());
    short_body.extend_from_slice(&0u32.to_le_bytes()); // no name bytes follow
    let r_short = rej(&short_body, PARSE_TRUNCATED);
    // Invalid UTF-8 in the app name.
    let mut bad_utf8 = Vec::new();
    bad_utf8.extend_from_slice(&MAGIC.to_le_bytes());
    bad_utf8.extend_from_slice(&1u32.to_le_bytes());
    bad_utf8.extend_from_slice(&2u32.to_le_bytes()); // name_len=2
    bad_utf8.extend_from_slice(&0u32.to_le_bytes());
    bad_utf8.extend_from_slice(&0u32.to_le_bytes());
    bad_utf8.extend_from_slice(&[0xFF, 0xFE]); // invalid UTF-8 name
    let r_utf8 = rej(&bad_utf8, PARSE_BAD_UTF8);

    let reject_ok = r_trunc && r_magic && r_ver && r_toolarge && r_short && r_utf8;
    let pass = happy_ok && reject_ok;
    crate::serial_println!(
        "[bundle] smoketest: parse+verify ok={}/bad={} happy={} reject(trunc={} magic={} ver={} large={} short={} utf8={}) -> {}",
        ok,
        bad,
        happy_ok,
        r_trunc,
        r_magic,
        r_ver,
        r_toolarge,
        r_short,
        r_utf8,
        if pass { "PASS" } else { "FAIL" },
    );
}
